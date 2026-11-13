//! Which checkout a path is in, and the build roots on the volume.
//!
//! Every checkout owns one build root, <root>/<repo>/<checkout>, where
//! <checkout> is `main` for the main checkout (the folder holding the common
//! .git) and the worktree's folder name (side-N) for any other. The root's
//! `.checkout` file names the checkout it belongs to; a root whose checkout is
//! gone, or is no longer that worktree, is gone too and prune deletes it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::text::sort_collated;

/// The variables `git rev-parse --local-env-vars` lists. A git hook runs the
/// tool with its own repository's GIT_DIR, GIT_INDEX_FILE and the rest
/// exported; kept, git answers for the hook's repository about every other
/// checkout, and each of another repo's build roots reads as gone. Every
/// command that runs git drops them first.
pub fn drop_git_local_env() -> Result<()> {
    for name in git_local_env_vars()? {
        // Runs before any thread starts.
        unsafe { std::env::remove_var(name) };
    }
    Ok(())
}

pub fn git_local_env_vars() -> Result<Vec<String>> {
    let mut command = Command::new("git");
    command.args(["rev-parse", "--local-env-vars"]);
    let listed = git_text(&mut command, "list its local variables")?;
    Ok(listed.split_whitespace().map(str::to_string).collect())
}

/// `git -C <dir>`.
pub fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir);
    command
}

/// stdout of a git command that must succeed, as text.
pub fn git_text(command: &mut Command, what: &str) -> Result<String> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running git to {what}"))?;
    if !output.status.success() {
        fail!(
            "git failed to {what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A git checkout: the main checkout or one of its worktrees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    /// Its working tree's top.
    pub top: PathBuf,
    /// The main checkout: the folder holding the common .git.
    pub main: PathBuf,
    /// Its own git dir (<main>/.git, or <main>/.git/worktrees/<name>).
    pub git_dir: PathBuf,
}

impl Checkout {
    /// The checkout <path> is in, or None when it is in none.
    pub fn at(path: &Path) -> Result<Option<Checkout>> {
        Checkout::asking_git(path, &[])
    }

    /// `at`, for a command that runs in its caller's environment as it is
    /// (`builds cargo`, whose cargo inherits it): git is asked without the
    /// caller's git variables, which a hook exports for its own repository,
    /// and this process keeps them.
    pub fn at_keeping_env(path: &Path) -> Result<Option<Checkout>> {
        Checkout::asking_git(path, &git_local_env_vars()?)
    }

    fn asking_git(path: &Path, unset: &[String]) -> Result<Option<Checkout>> {
        let mut command = git(path);
        for name in unset {
            command.env_remove(name);
        }
        let output = command
            .args([
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-common-dir",
                "--absolute-git-dir",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .context(|| format!("running git in {}", path.display()))?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines = text.lines();
        let (Some(top), Some(common), Some(git_dir)) = (lines.next(), lines.next(), lines.next())
        else {
            fail!("git rev-parse in {} printed {text:?}", path.display());
        };
        let common = PathBuf::from(common);
        let Some(main) = common.parent() else {
            fail!("git's common dir {} has no parent", common.display());
        };
        Ok(Some(Checkout {
            top: PathBuf::from(top),
            main: main.to_path_buf(),
            git_dir: PathBuf::from(git_dir),
        }))
    }

    /// The checkout <arg> names; fails when it is inside none.
    pub fn find(arg: &str) -> Result<Checkout> {
        match Checkout::at(Path::new(arg))? {
            Some(checkout) => Ok(checkout),
            None => fail!("{arg} is not inside a git checkout"),
        }
    }

    pub fn repo(&self) -> String {
        file_name(&self.main)
    }

    pub fn is_main(&self) -> bool {
        self.top == self.main
    }

    /// Is this one of the repo's own side worktrees,
    /// <main's folder>/../<repo>-worktrees/side-N? The one definition of what
    /// `builds` hands out, retires, claims and judges; a worktree anything
    /// else made (Claude Code's harness puts agents' under
    /// <main>/.claude/worktrees) is not one, and no command treats it as one.
    pub fn is_side(&self) -> bool {
        let Some(parent) = self.main.parent() else { return false };
        let folder = parent.join(format!("{}-worktrees", self.repo()));
        self.top.parent() == Some(folder.as_path())
            && file_name(&self.top)
                .strip_prefix("side-")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }

    /// `main`, or the worktree's folder name.
    pub fn name(&self) -> String {
        if self.is_main() { "main".to_string() } else { file_name(&self.top) }
    }

    pub fn build_root(&self, places: &Places) -> PathBuf {
        places.root.join(self.repo()).join(self.name())
    }

    /// A claim is a file in the worktree's own git dir, so it goes when the
    /// worktree does.
    pub fn claim_file(&self) -> PathBuf {
        self.git_dir.join("builds-claim")
    }

    /// The branch checked out, or None when HEAD is detached.
    pub fn branch(&self) -> Result<Option<String>> {
        let output = git(&self.top)
            .args(["branch", "--show-current"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .context(|| format!("running git in {}", self.top.display()))?;
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(if branch.is_empty() { None } else { Some(branch) })
    }

    /// A repo's folder sits beside DerivedData, caches and .trash at the
    /// volume's root, so a repo can't take one of their names.
    pub fn require_repo_name(&self, places: &Places) -> Result<()> {
        let repo = self.repo();
        if places.is_reserved_name(&repo) {
            fail!(
                "repo {repo} would build into {}, which builds keeps for something else",
                places.root.join(&repo).display()
            );
        }
        Ok(())
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A build root on the volume and what its `.checkout` says.
#[derive(Clone, Debug)]
pub struct Root {
    /// <root>/<repo>/<checkout>
    pub path: PathBuf,
    /// The checkout `.checkout` names.
    pub top: PathBuf,
    /// That checkout, while it is still a worktree whose build root is this
    /// one; None when it is gone.
    pub live: Option<Checkout>,
}

impl Root {
    /// The root at <path>, or None when it has no `.checkout`.
    pub fn read(places: &Places, path: &Path) -> Result<Option<Root>> {
        let marker = path.join(".checkout");
        let text = match std::fs::read_to_string(&marker) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => fail!("reading {}: {error}", marker.display()),
        };
        let top = PathBuf::from(text.trim_end_matches('\n'));
        let live = if top.is_dir() {
            Checkout::at(&top)?
                .filter(|checkout| checkout.top == top && checkout.build_root(places) == path)
        } else {
            None
        };
        Ok(Some(Root { path: path.to_path_buf(), top, live }))
    }

    pub fn name(&self) -> String {
        file_name(&self.path)
    }

    /// Is this main's or an app checkout's root? Their builds stay warm on
    /// purpose: the person installs from main and tests agents' app builds in
    /// the app checkouts. The free-space floor never deletes their folders (an
    /// app checkout's folders of the other lane excepted, see
    /// `BuildDir::is_app_in`); the caps still apply to them.
    pub fn keeps_warm(&self, places: &Places) -> Result<bool> {
        keeps_warm(places, &self.path)
    }

    /// The lane this is its repo's app checkout of (main keeps its builds warm
    /// too, but is not one).
    pub fn app_lane(&self, places: &Places) -> Result<Option<Lane>> {
        app_lane(places, &self.path)
    }

    /// Is this an app checkout of either lane?
    pub fn is_app_checkout(&self, places: &Places) -> Result<bool> {
        Ok(self.app_lane(places)?.is_some())
    }
}

/// See `Root::app_lane`; for a root path.
pub fn app_lane(places: &Places, root: &Path) -> Result<Option<Lane>> {
    let checkout = file_name(root);
    if checkout == "main" {
        return Ok(None);
    }
    let repo = root.parent().map(file_name).unwrap_or_default();
    places.app_lane_of(&repo, &checkout)
}

/// See `Root::keeps_warm`; for a root path.
pub fn keeps_warm(places: &Places, root: &Path) -> Result<bool> {
    Ok(file_name(root) == "main" || app_lane(places, root)?.is_some())
}

/// Every build root on the volume (<root>/*/*/.checkout), in glob order.
pub fn all_root_paths(places: &Places) -> Result<Vec<PathBuf>> {
    let mut markers = Vec::new();
    for repo in visible_dirs(&places.root)? {
        for checkout in visible_dirs(&repo)? {
            if checkout.join(".checkout").is_file() {
                markers.push(checkout.join(".checkout"));
            }
        }
    }
    sort_collated(&mut markers);
    Ok(markers
        .into_iter()
        .map(|marker| marker.parent().expect("a marker has a folder").to_path_buf())
        .collect())
}

/// Every build root, each read (git runs for each, so several at a time).
pub fn all_roots(places: &Places) -> Result<Vec<Root>> {
    let paths = all_root_paths(places)?;
    let read = parallel_map(&paths, |path| Root::read(places, path));
    let mut roots = Vec::new();
    for root in read {
        if let Some(root) = root? {
            roots.push(root);
        }
    }
    Ok(roots)
}

fn visible_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", dir.display()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", dir.display()))?;
        if entry.file_name().as_encoded_bytes().first() == Some(&b'.') {
            continue;
        }
        // `*/*/.checkout` follows links, as a glob does.
        if entry.path().is_dir() {
            found.push(entry.path());
        }
    }
    Ok(found)
}

/// <f> over <items>, eight at a time, results in order.
pub fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<R>>> =
        items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..8.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= items.len() {
                        break;
                    }
                    let result = f(&items[i]);
                    *results[i].lock().expect("no thread panicked holding it") = Some(result);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .expect("no thread panicked holding it")
                .expect("every item ran")
        })
        .collect()
}

/// The absolute path a symlink at <link> points at.
pub fn absolute_link_target(link: &Path) -> Result<PathBuf> {
    let target = std::fs::read_link(link).context(|| format!("reading link {}", link.display()))?;
    if target.is_absolute() {
        return Ok(target);
    }
    let parent = link.parent().expect("a link has a folder");
    let parent = std::fs::canonicalize(parent).context(|| format!("resolving {}", parent.display()))?;
    Ok(parent.join(target))
}

/// The worktrees of <top>'s repo, as `git worktree list` gives them.
pub fn worktrees_of(top: &Path) -> Result<Vec<PathBuf>> {
    let text = git_text(git(top).args(["worktree", "list", "--porcelain"]), "list worktrees")?;
    Ok(text
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect())
}
