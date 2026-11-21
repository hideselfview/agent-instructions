//! App checkouts on a host. A lane's app checkout can live on another Mac:
//! `builds app-checkout android --host m1 <path>` records it as the entry
//! `m1:<name>` in the lane's set, beside the entries of the checkouts on this
//! Mac. The lock, the queue and the record of what the checkout last built
//! stay on this Mac, under the entry's name, so the queue, takeovers, the
//! dashboard and `builds health` treat a checkout on a host as any other.
//!
//! The checkout on the host is the worktree `~/dev/<repo>-worktrees/<name>` of
//! the host's clone of the repo, set up as a side worktree is here: made with
//! `git worktree add`, then the host's `builds setup` and the repo's
//! `scripts/hooks/post-checkout`, with the host's builds root
//! (`/Volumes/Builds` when the host has it, else `~/builds-root.noindex`) so
//! the build folders live under `<root>/<repo>/<name>`, and, in the android
//! lane, a `local.properties` pointing each Gradle project at the host's
//! Android SDK.
//! A build sends its commit to the host's clone as a job ref
//! (`refs/builds/jobs/<id>`, never to GitHub), switches the checkout to it
//! (the checkout must have no changes, as here) and deletes the ref; the
//! checkout's detached HEAD keeps the commit.
//!
//! Each step runs there through `builds remote step`, a process of this
//! Mac that `lock::run_in_group_within` times and stops like a local step: its
//! output comes back as the step prints it, and a time limit or a takeover
//! stops the step on the host.
//!
//! The repo's `results <lane> <path>` lines in `.app-build` name the folders
//! of the checkout that agents read after a run of tests or screenshots; they
//! are copied back to `<root>/<repo>/<entry>/<path>` on this Mac (the folder
//! of the checkout's build output, where a local checkout's `<path>` links to).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::app_config::{app_checkout_top, run_git};
use crate::awake::KeepAwake;
use crate::checkout::{APP_LANE_FILE, Checkout};
use crate::fail;
use crate::fail::{Context, Result};
use crate::host::{self, Host, Ssh, clone_path, sh_quote};
use crate::lane::Lane;
use crate::places::Places;
use crate::remote::{
    JOB_WORKTREE_PREFIX, JOBS_REF, Transport, job_id, release_script, root_script, worktree_in,
};

/// What separates the host from the checkout's name in a lane's entry.
const SEPARATOR: char = ':';

/// The host and the name in the entry <entry>, when it is a checkout on a
/// host rather than one of this Mac's side worktrees.
pub fn split(entry: &str) -> Option<(&str, &str)> {
    entry.split_once(SEPARATOR)
}

/// The entry for the checkout <name> on <host>.
pub fn entry_of(host: &str, name: &str) -> String {
    format!("{host}{SEPARATOR}{name}")
}

/// The host and the name that <path> (a folder on the host, as `builds
/// app-checkout --host` is given it) names in <repo>'s worktrees folder there:
/// `.../dev/<repo>-worktrees/<name>`.
pub fn name_in(path: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let [.., dev, folder, name] = parts[..] else {
        fail!(
            "{path:?} is not a folder of a host's app checkout; it is ~/dev/<repo>-worktrees/<name> there"
        );
    };
    let Some(repo) = folder
        .strip_suffix("-worktrees")
        .filter(|repo| !repo.is_empty())
    else {
        fail!(
            "{path:?} is not in a <repo>-worktrees folder; a host's app checkout is ~/dev/<repo>-worktrees/<name>"
        );
    };
    if dev != "dev" {
        fail!("{path:?} is not under dev/; a host's app checkout is ~/dev/<repo>-worktrees/<name>");
    }
    if name.starts_with(JOB_WORKTREE_PREFIX) {
        fail!(
            "{name} is a name `builds remote run` gives its job worktrees; give the app checkout another"
        );
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        fail!("{name:?} must be letters, digits, dashes and underscores");
    }
    Ok((repo.to_string(), name.to_string()))
}

/// Where a build reads its commits from on this Mac: the checkout it runs in
/// when that is a checkout of <repo>, else the first of <repo>'s app
/// checkouts here, else the clone `~/dev/<repo>`.
pub fn local_source(places: &Places, repo: &str) -> Result<PathBuf> {
    let here = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    if let Some(checkout) = Checkout::at(&here)?
        && checkout.repo() == repo
    {
        return Ok(checkout.top);
    }
    for lane in Lane::ALL {
        for entry in places.app_checkouts_of(repo, lane)? {
            if split(&entry).is_none() {
                return app_checkout_top(places, repo, &entry);
            }
        }
    }
    let clone = places.home.join(clone_path(repo));
    if !clone.join(".git").exists() {
        fail!(
            "{repo} has no app checkout here and no clone at {} to read commits from",
            clone.display()
        );
    }
    Ok(clone)
}

/// `local_source`, after a fetch from origin there, so that it holds the
/// commits origin has now.
pub fn fetched_local_source(places: &Places, repo: &str) -> Result<PathBuf> {
    let source = local_source(places, repo)?;
    run_git(&source, &["fetch", "-q", "origin"])?;
    Ok(source)
}

/// <command>, run under `caffeinate -d -u` in the mac lane, which holds a
/// display assertion on the host for as long as the command runs, so the
/// host's display neither sleeps nor locks during a step that opens windows
/// (host_screen_lock.rs). Another lane's <command> stays as it is.
pub fn keeping_display_on(lane: Lane, command: &[String]) -> Vec<String> {
    if !lane.uses_xcode() {
        return command.to_vec();
    }
    ["/usr/bin/caffeinate", "-d", "-u"]
        .iter()
        .map(|word| word.to_string())
        .chain(command.iter().cloned())
        .collect()
}

/// The commands that make, switch and read the checkout `<name>` of <repo> on
/// a host. While a `RemoteApp` exists, this Mac stays awake (awake.rs): its
/// steps and copies run over ssh, and a sleep here ends their connections.
#[derive(Debug)]
pub struct RemoteApp {
    places: Places,
    ssh: Ssh,
    pub host: Host,
    repo: String,
    name: String,
    lane: Lane,
    /// The checkout's folder on the host, once `switch_to` or `locate` has
    /// asked.
    worktree: Option<String>,
    _awake: KeepAwake,
}

impl RemoteApp {
    /// The checkout <entry> of <repo>'s <lane>, when it is on a host.
    pub fn find(
        places: &Places,
        ssh: &Ssh,
        repo: &str,
        lane: Lane,
        entry: &str,
    ) -> Result<Option<RemoteApp>> {
        let Some((host_name, name)) = split(entry) else {
            return Ok(None);
        };
        let host = host::remote_named(places, entry, host_name)?;
        let awake = KeepAwake::for_host(&host, &format!("{repo}'s app checkout {name}"))?;
        Ok(Some(RemoteApp {
            places: places.clone(),
            ssh: ssh.clone(),
            host,
            repo: repo.to_string(),
            name: name.to_string(),
            lane,
            worktree: None,
            _awake: awake,
        }))
    }

    fn transport(&self) -> Transport<'_> {
        Transport {
            places: &self.places,
            ssh: &self.ssh,
            host: &self.host,
        }
    }

    /// The folder on this Mac where the checkout's lock, its record of what it
    /// built and the results copied back live.
    pub fn local_folder(&self) -> PathBuf {
        self.places
            .root
            .join(&self.repo)
            .join(entry_of(&self.host.name, &self.name))
    }

    /// The checkout's folder on the host; it need not exist yet.
    pub fn locate(&mut self) -> Result<String> {
        if let Some(worktree) = &self.worktree {
            return Ok(worktree.clone());
        }
        let said = self.transport().capture(
            &format!(
                "echo \"worktree=$HOME/dev/{}-worktrees/{}\"",
                self.repo, self.name
            ),
            "finding the app checkout",
        )?;
        let worktree = worktree_in(&said)?;
        self.worktree = Some(worktree.clone());
        Ok(worktree)
    }

    /// The commit the checkout is on, or "" when the host has none yet.
    pub fn head(&mut self) -> Result<String> {
        let worktree = self.locate()?;
        let said = self.transport().capture(
            &format!(
                "git -C {} rev-parse HEAD 2>/dev/null || true",
                sh_quote(&worktree)
            ),
            "reading the app checkout's commit",
        )?;
        Ok(said.trim().to_string())
    }

    /// Sends <sha> from <source> to the host's clone, makes the checkout there
    /// when it has none (set up as a side worktree is) and switches it to
    /// <sha>. Fails, naming the host, when the checkout has changes.
    pub fn switch_to(&mut self, source: &Path, sha: &str) -> Result<()> {
        let id = self.send(source, sha)?;
        let made = self.transport().capture(
            &ensure_script(&self.host, &self.repo, &self.name, sha, self.lane),
            "switching the app checkout",
        );
        let dropped = self.unsend(&id);
        let made = crate::fail::with_cleanup(made, dropped)?;
        self.worktree = Some(worktree_in(&made)?);
        Ok(())
    }

    /// Runs git with <args> in the checkout on the host, with the index file
    /// <index> when given, and returns what it printed; a failure names the
    /// host and what git said.
    pub fn git(&mut self, index: Option<&str>, args: &[&str]) -> Result<String> {
        let index = index
            .map(|index| format!("GIT_INDEX_FILE={} ", sh_quote(index)))
            .unwrap_or_default();
        let quoted: Vec<String> = args.iter().map(|arg| sh_quote(arg)).collect();
        self.run_in(
            &format!("{index}git {}", quoted.join(" ")),
            &format!("git {}", args.join(" ")),
        )
    }

    /// Runs <script> with `sh` in the checkout's folder on the host and
    /// returns what it printed; a failure names the host and <what>.
    pub fn run_in(&mut self, script: &str, what: &str) -> Result<String> {
        let worktree = self.locate()?;
        self.transport()
            .capture(&format!("cd {} && {script}", sh_quote(&worktree)), what)
    }

    /// Sends <sha> from <source> to the host's clone as the ref of a new job,
    /// so that git there can read it, and returns the job's id; `unsend`
    /// deletes the ref.
    pub fn send(&self, source: &Path, sha: &str) -> Result<String> {
        let id = job_id();
        self.transport().send(source, &self.repo, sha, &id)?;
        Ok(id)
    }

    /// Deletes the ref that `send` made for job <id>.
    pub fn unsend(&self, id: &str) -> Result<()> {
        self.transport()
            .capture(
                &release_script(&self.repo, None, id),
                "removing the job's ref",
            )
            .map(|_| ())
    }

    /// Removes the checkout from the host, with its build folders there, so
    /// the host gets back the disk space they used. When the host has no such
    /// checkout, `remove` succeeds and does nothing. The caller holds the
    /// checkout's lock, so no build runs in it. A failure names the host and
    /// what the removal printed there.
    pub fn remove(&self) -> std::result::Result<(), String> {
        let script = REMOVE
            .replace("@ROOT@", &root_script(&self.host))
            .replace("@CLONE@", &clone_path(&self.repo))
            .replace("@REPO@", &self.repo)
            .replace("@NAME@", &self.name);
        match self.transport().ask(&script, "removing the app checkout") {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(said)) => Err(said),
            Err(error) => Err(error.text()),
        }
    }

    /// Writes the lane to the checkout's lane file on the host
    /// (`Checkout::app_lane_file`), so the host's builds keeps the checkout's
    /// builds warm, as a person adds it to the lane. When the host has no such
    /// worktree yet, `mark` does nothing: the first build makes the checkout
    /// and writes the file before the host's setup runs. Returns whether it
    /// wrote the file. A failure, such as a host that does not answer, names
    /// the host and what the script printed there.
    pub fn mark(&self) -> Result<bool> {
        marked_in(&self.lane_file_script(MARK, "writing the app checkout's lane file")?)
    }

    /// Deletes the checkout's lane file on the host, so the host's builds
    /// stops keeping the checkout's builds warm once it is no lane's. The
    /// worktree stays. When the host has no such worktree, `unmark` does
    /// nothing. A failure names the host, as `mark`'s does.
    pub fn unmark(&self) -> Result<()> {
        self.lane_file_script(UNMARK, "deleting the app checkout's lane file")
            .map(|_| ())
    }

    /// Runs <script> on the host and returns what it printed.
    fn lane_file_script(&self, script: &str, what: &str) -> Result<String> {
        let script = script
            .replace("@MARK_LANE@", MARK_LANE)
            .replace("@REPO@", &self.repo)
            .replace("@NAME@", &self.name)
            .replace("@LANE@", self.lane.word())
            .replace("@LANE_FILE@", APP_LANE_FILE);
        self.transport().capture(&script, what)
    }

    /// <command> for a step that opens windows, as `keeping_display_on`
    /// makes it for this checkout's lane.
    pub fn keeping_display_on(&self, command: &[String]) -> Vec<String> {
        keeping_display_on(self.lane, command)
    }

    /// The command that runs <command> in the checkout on the host and is
    /// stopped as a step here is: a `builds remote step` process.
    pub fn step_command(&mut self, command: &[String]) -> Result<Command> {
        let worktree = self.locate()?;
        let exe = crate::this_program()?;
        let mut start = Command::new(exe);
        start
            .args(["remote", "step", &self.host.name, &worktree, "--"])
            .args(command);
        Ok(start)
    }

    /// <command>, run under the build's builds <pinned> on the host:
    /// `<folder>/builds programs hold -- <command>`, where <folder> is the
    /// host's folder of its version (programs.rs). The function copies the
    /// builds of this Mac's folder of the version when the host's folder
    /// lacks it.
    pub fn with_programs(
        &mut self,
        pinned: &crate::programs::Pinned,
        command: &[String],
    ) -> Result<Vec<String>> {
        let (version, local) = (pinned.version(), pinned.dir());
        let said = self.transport().capture(
            &format!(
                r#"dir="$HOME/{folder}/{version}"
mkdir -p "$dir"
if [ -x "$dir/builds" ]; then echo "present=$dir"; else echo "absent=$dir"; fi"#,
                folder = crate::programs::FOLDER,
            ),
            "finding this builds' programs",
        )?;
        let (present, there) = match said.lines().find_map(|line| {
            line.strip_prefix("present=")
                .map(|dir| (true, dir))
                .or_else(|| line.strip_prefix("absent=").map(|dir| (false, dir)))
        }) {
            Some((present, dir)) => (present, dir.to_string()),
            None => fail!(
                "host {} did not say where this builds' programs are: {said}",
                self.host.name
            ),
        };
        if !present {
            let mut cat = Command::new("cat");
            cat.arg(local.join("builds"));
            let into = format!("{there}/builds");
            let staged = format!("{there}/.builds.{}", crate::text::staged_suffix());
            crate::remote::pipe(
                cat,
                self.transport().command(&format!(
                    "cat > {staged} && chmod 755 {staged} && mv -f {staged} {into}",
                    staged = sh_quote(&staged),
                    into = sh_quote(&into),
                )),
                &format!(
                    "{} to host {}",
                    local.join("builds").display(),
                    self.host.name
                ),
            )?;
        }
        let mut held = vec![
            format!("{there}/builds"),
            "programs".to_string(),
            "hold".to_string(),
            "--".to_string(),
        ];
        held.extend(command.iter().cloned());
        Ok(held)
    }

    /// Copies the file <local> here to a new temporary file on the host and
    /// returns its path there; `remove_temporary` deletes it.
    pub fn put_temporary(&mut self, local: &Path) -> Result<String> {
        let made = self.transport().capture(
            "mktemp \"${TMPDIR:-/tmp}/builds-before.XXXXXX\"",
            "making a temporary file",
        )?;
        let there = made.trim().to_string();
        let mut cat = Command::new("cat");
        cat.arg(local);
        crate::remote::pipe(
            cat,
            self.transport()
                .command(&format!("cat > {}", sh_quote(&there))),
            &format!("{} to host {}", local.display(), self.host.name),
        )?;
        Ok(there)
    }

    /// Copies the parts <parts> of the folder <local> that exist (folders in
    /// it) to a new temporary folder on the host, and returns the folder's
    /// path there.
    pub fn put_folder(&mut self, local: &Path, parts: &[String]) -> Result<String> {
        let made = self.transport().capture(
            "mktemp -d \"${TMPDIR:-/tmp}/builds-pictures.XXXXXX\"",
            "making a temporary folder",
        )?;
        let there = made.trim().to_string();
        let present: Vec<&String> = parts
            .iter()
            .filter(|part| local.join(part).is_dir())
            .collect();
        if present.is_empty() {
            return Ok(there);
        }
        let mut tar = Command::new("tar");
        tar.arg("-cf").arg("-").arg("-C").arg(local).args(present);
        crate::remote::pipe(
            tar,
            self.transport()
                .command(&format!("tar -xf - -C {}", sh_quote(&there))),
            &format!("{} to host {}", local.display(), self.host.name),
        )?;
        Ok(there)
    }

    /// Copies the folder <there> on the host into the new folder <into>
    /// here.
    pub fn fetch_folder(&mut self, there: &str, into: &Path) -> Result<()> {
        std::fs::create_dir_all(into).context(|| format!("creating {}", into.display()))?;
        crate::remote::unpack(
            self.transport()
                .command(&format!("cd {} && tar -cf - .", sh_quote(there))),
            into,
            &format!("{there} from host {}", self.host.name),
        )
    }

    /// Deletes the temporary file or folder <there> that `put_temporary` or
    /// `put_folder` made on the host.
    pub fn remove_temporary(&mut self, there: &str) -> Result<()> {
        self.transport()
            .capture(
                &format!("rm -rf {}", sh_quote(there)),
                &format!("deleting {there}"),
            )
            .map(|_| ())
    }

    /// Copies the shots report in <dir> (a folder of the checkout on the
    /// host: its `index.html`, the files that page shows and its findings as
    /// records, when it has them) into the new folder <into> here.
    pub fn fetch_report(&mut self, dir: &str, into: &Path) -> Result<()> {
        let html = self.transport().capture(
            &format!("cat {}", sh_quote(&format!("{dir}/index.html"))),
            "reading the shots report",
        )?;
        std::fs::create_dir_all(into).context(|| format!("creating {}", into.display()))?;
        let files = crate::shot_findings::report_files(&html);
        crate::remote::unpack(
            self.transport()
                .command(&crate::shot_findings::report_copy_script(dir, &files)),
            into,
            &format!("the shots report from host {}", self.host.name),
        )
    }

    /// Copies each of <paths> (folders of the checkout) from the host to
    /// `local_folder()/<path>`, replacing what is there; a path the checkout
    /// has not made is left out.
    pub fn pull(&mut self, paths: &[String]) -> Result<()> {
        self.pull_with(paths, "-chf")
    }

    /// `pull` for a built app: symbolic links stay links, since a bundle's
    /// frameworks hold some and a copy of what they point at breaks its
    /// signature.
    pub fn pull_bundle(&mut self, path: &str) -> Result<()> {
        self.pull_with(&[path.to_string()], "-cf")
    }

    fn pull_with(&mut self, paths: &[String], tar_flags: &str) -> Result<()> {
        let worktree = self.locate()?;
        let into = self.local_folder();
        std::fs::create_dir_all(&into).context(|| format!("creating {}", into.display()))?;
        for path in paths {
            let target = into.join(path);
            if target.is_dir() {
                std::fs::remove_dir_all(&target)
                    .context(|| format!("removing {}", target.display()))?;
            }
            let script = format!(
                "cd {} && if [ -e {path} ]; then tar {tar_flags} - {path}; fi",
                sh_quote(&worktree),
                path = sh_quote(path)
            );
            crate::remote::unpack(
                self.transport().command(&script),
                &into,
                &format!("{path} from host {}", self.host.name),
            )?;
        }
        Ok(())
    }
}

/// The script that makes the checkout <name> of <repo> on the host when it
/// has none, set up as a side worktree is, or switches it to <sha> when it
/// has no changes; it prints `worktree=<folder>`. A new checkout's setup
/// writes <lane> to the checkout's lane file (`Checkout::app_lane_file`),
/// which the host's builds reads to keep the checkout's builds warm (a
/// checkout the host already had got the file when a person added it to the
/// lane, `RemoteApp::mark`), then runs the host's `builds setup`, which links
/// the build folders into the host's builds root, and then the repo's
/// post-checkout, as a job's worktree gets
/// (remote.rs `claim_script`): a repo with no post-checkout would otherwise
/// build into real folders in the checkout, outside the build volume and
/// every cap. A checkout whose setup fails is removed, so the next build sets
/// it up again rather than finding half of it.
fn ensure_script(host: &Host, repo: &str, name: &str, sha: &str, lane: Lane) -> String {
    let android = match lane {
        Lane::Android => ANDROID_SETUP,
        Lane::Mac => "",
    };
    ENSURE
        .replace("@MARK_LANE@", MARK_LANE)
        .replace("@ROOT@", &root_script(host))
        .replace("@CLONE@", &clone_path(repo))
        .replace("@REPO@", repo)
        .replace("@NAME@", name)
        .replace("@SHA@", &sh_quote(sha))
        .replace("@JOBS@", JOBS_REF)
        .replace("@ANDROID@", android)
        .replace("@LANE@", lane.word())
        .replace("@LANE_FILE@", APP_LANE_FILE)
}

/// What a lane's checkout needs besides the repo's post-checkout: in the
/// android lane, Gradle's `local.properties` naming the host's SDK in each
/// Gradle project (a folder with a `gradlew`, at the top or one folder down).
const ANDROID_SETUP: &str = r#"  sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
  if [ ! -d "$sdk" ]; then echo "no Android SDK at $sdk"; return 1; fi
  for gradlew in gradlew */gradlew; do
    [ -f "$gradlew" ] || continue
    dir=$(dirname "$gradlew")
    if [ ! -e "$dir/local.properties" ]; then
      echo "sdk.dir=$sdk" > "$dir/local.properties" || return 1
    fi
  done
"#;

const ENSURE: &str = r#"@ROOT@set -e
clone="$HOME/@CLONE@"
if [ ! -e "$clone/.git" ]; then echo "no clone of @REPO@ at $clone" >&2; exit 90; fi
wts="$HOME/dev/@REPO@-worktrees"
wt="$wts/@NAME@"
mkdir -p "$wts" "$wt.job"
@MARK_LANE@
setup() {
  cd "$wt" || return 1
  mark_lane || return 1
  "$HOME/.local/bin/builds" setup "$wt" || return 1
  if [ -x scripts/hooks/post-checkout ]; then
    scripts/hooks/post-checkout 0000000000000000000000000000000000000000 @SHA@ 1 || return 1
  fi
@ANDROID@}
# The post-checkout links the build folders into the builds root; the main
# checkout here ignores those links in .git/info/exclude (which the host's clone
# lacks), so the host's clone ignores each one a worktree shows as new.
exclude_links() {
  exclude="$(git -C "$wt" rev-parse --git-common-dir)/info/exclude"
  case "$exclude" in /*) ;; *) exclude="$wt/$exclude" ;; esac
  mkdir -p "$(dirname "$exclude")"
  git -C "$wt" ls-files --others --exclude-standard | while IFS= read -r path; do
    if [ -L "$wt/$path" ]; then echo "/$path" >> "$exclude"; fi
  done
}
if [ -e "$wt/.git" ]; then
  exclude_links
  if [ -n "$(git -C "$wt" status --porcelain)" ]; then
    echo "the app checkout $wt has changes, so nothing was built:" >&2
    git -C "$wt" status --porcelain >&2
    exit 94
  fi
  git -C "$wt" checkout -q --detach @SHA@
else
  git -C "$clone" worktree add -q --detach "$wt" @SHA@
  if ! said=$(setup 2>&1); then
    echo "setting up $wt failed: $said" >&2
    git -C "$clone" worktree remove --force "$wt"
    exit 93
  fi
  exclude_links
fi
echo "worktree=$wt""#;

/// The shell function that writes @LANE@ to the lane file of the checkout
/// "$wt"; it fails when git cannot name the checkout's git dir.
const MARK_LANE: &str = r#"mark_lane() {
  git_dir=$(git -C "$wt" rev-parse --absolute-git-dir) || return 1
  printf '%s\n' @LANE@ > "$git_dir/@LANE_FILE@"
}"#;

/// The script that writes the lane file of the checkout <name> of <repo> on
/// the host, when the host has the worktree, and says which it did
/// (`marked_in`).
const MARK: &str = r#"@MARK_LANE@
wt="$HOME/dev/@REPO@-worktrees/@NAME@"
if [ -e "$wt/.git" ]; then
  mark_lane || exit 1
  echo lane-file=written
else
  echo lane-file=no-worktree
fi"#;

/// Whether `MARK` wrote the lane file, from the last `lane-file=` line it
/// printed: `written`, or `no-worktree` when the host has no such worktree.
/// Fails on any other output.
fn marked_in(output: &str) -> Result<bool> {
    match output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("lane-file="))
    {
        Some("written") => Ok(true),
        Some("no-worktree") => Ok(false),
        _ => fail!("the host did not say whether it wrote the lane file: {output:?}"),
    }
}

/// The script that deletes the lane file of the checkout <name> of <repo> on
/// the host, when the host has the worktree.
const UNMARK: &str = r#"set -e
wt="$HOME/dev/@REPO@-worktrees/@NAME@"
if [ -e "$wt/.git" ]; then
  git_dir=$(git -C "$wt" rev-parse --absolute-git-dir)
  rm -f "$git_dir/@LANE_FILE@"
fi"#;

/// The script that removes the checkout <name> of <repo> on the host: the
/// worktree from the host's clone, the folder `ENSURE` makes beside it, and
/// its build folders under the host's builds root.
const REMOVE: &str = r#"@ROOT@clone="$HOME/@CLONE@"
wt="$HOME/dev/@REPO@-worktrees/@NAME@"
if [ -e "$wt/.git" ]; then git -C "$clone" worktree remove --force "$wt" || exit 1; fi
rm -rf "$wt" "$wt.job" "$BUILDS_ROOT/@REPO@/@NAME@" || exit 1
if [ -e "$clone/.git" ]; then git -C "$clone" worktree prune; fi"#;

#[cfg(test)]
#[path = "app_remote_tests.rs"]
mod tests;
