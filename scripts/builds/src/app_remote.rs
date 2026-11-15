//! App checkouts on a host. A lane's app checkout can live on another Mac:
//! `builds app-checkout android --host m1 <path>` records it as the entry
//! `m1:<name>` in the lane's set, beside the entries of the checkouts on this
//! Mac. The lock, the queue and the record of what the checkout last built
//! stay on this Mac, under the entry's name, so the queue, takeovers, the
//! dashboard and `builds health` treat a checkout on a host as any other.
//!
//! The checkout on the host is the worktree `~/dev/<repo>-worktrees/<name>` of
//! the host's clone of the repo, set up as a side worktree is here: made with
//! `git worktree add` and then the repo's `scripts/hooks/post-checkout`, with
//! the host's builds root (`/Volumes/Builds` when the host has it, else
//! `~/builds-root`) so the build folders live under `<root>/<repo>/<name>`, and, in the android lane, a
//! `local.properties` pointing each Gradle project at the host's Android SDK.
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
use std::process::{Command, Stdio};

use crate::app_config::app_checkout_top;
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::host::{self, Host, Ssh, clone_path, sh_quote};
use crate::lane::Lane;
use crate::places::Places;
use crate::remote::{JOBS_REF, Transport, job_id, release_script, root_script, worktree_in};

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
    if name.starts_with("remote-") {
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

/// The commands that make, switch and read the checkout `<name>` of <repo> on
/// a host.
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
        let Some(host) = host::read_remote(places)?
            .into_iter()
            .find(|host| host.name == host_name)
        else {
            fail!("{entry} is on host {host_name}, which `builds host list` does not have");
        };
        Ok(Some(RemoteApp {
            places: places.clone(),
            ssh: ssh.clone(),
            host,
            repo: repo.to_string(),
            name: name.to_string(),
            lane,
            worktree: None,
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

    /// Where this Mac reads the repo's commits from (`local_source`).
    pub fn source(&self) -> Result<PathBuf> {
        local_source(&self.places, &self.repo)
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
        let id = job_id();
        let transport = self.transport();
        transport.send(source, &self.repo, sha, &id)?;
        let made = transport.capture(
            &ensure_script(&self.host, &self.repo, &self.name, sha, self.lane),
            "switching the app checkout",
        );
        let dropped = transport.capture(
            &release_script(&self.repo, None, &id),
            "removing the job's ref",
        );
        let worktree = worktree_in(&made?)?;
        dropped?;
        self.worktree = Some(worktree);
        Ok(())
    }

    /// The command that runs <command> in the checkout on the host and is
    /// stopped as a step here is: a `builds remote step` process.
    pub fn step_command(&mut self, command: &[String]) -> Result<Command> {
        let worktree = self.locate()?;
        let exe = std::env::current_exe().context(|| "finding the builds program".to_string())?;
        let mut start = Command::new(exe);
        start
            .args(["remote", "step", &self.host.name, &worktree, "--"])
            .args(command);
        Ok(start)
    }

    /// Copies the shots report in <dir> (a folder of the checkout on the
    /// host: its `index.html` and the files that page shows) to the folder
    /// <into> here, replacing what is there.
    pub fn fetch_report(&mut self, dir: &str, into: &Path) -> Result<()> {
        let html = self.transport().capture(
            &format!("cat {}", sh_quote(&format!("{dir}/index.html"))),
            "reading the shots report",
        )?;
        let staging = crate::shot_findings::staging_of(into);
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).context(|| format!("creating {}", staging.display()))?;
        std::fs::write(staging.join("index.html"), &html)
            .context(|| format!("writing {}", staging.join("index.html").display()))?;
        let files = crate::shot_findings::report_files(&html);
        if !files.is_empty() {
            let script = format!(
                "cd {} && tar -chf - {}",
                sh_quote(dir),
                files
                    .iter()
                    .map(|file| sh_quote(file))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let mut sender = self
                .transport()
                .command(&script)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context(|| format!("copying the shots report from host {}", self.host.name))?;
            let archive = sender.stdout.take().expect("stdout was piped");
            let received = Command::new("tar")
                .args(["-xf", "-", "-C"])
                .arg(&staging)
                .stdin(Stdio::from(archive))
                .output()
                .context(|| "running tar".to_string())?;
            let sent = sender
                .wait_with_output()
                .context(|| format!("waiting for host {}", self.host.name))?;
            if !sent.status.success() {
                fail!(
                    "copying the shots report from host {} failed ({}): {}",
                    self.host.name,
                    sent.status,
                    String::from_utf8_lossy(&sent.stderr).trim()
                );
            }
            if !received.status.success() {
                fail!(
                    "unpacking the shots report from host {} failed: {}",
                    self.host.name,
                    String::from_utf8_lossy(&received.stderr).trim()
                );
            }
        }
        crate::shot_findings::install_folder(&staging, into)
    }

    /// Copies each of <paths> (folders of the checkout) from the host to
    /// `local_folder()/<path>`, replacing what is there; a path the checkout
    /// has not made is left out.
    pub fn pull(&mut self, paths: &[String]) -> Result<()> {
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
                "cd {} && if [ -e {path} ]; then tar -chf - {path}; fi",
                sh_quote(&worktree),
                path = sh_quote(path)
            );
            let mut sender = self
                .transport()
                .command(&script)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context(|| format!("copying {path} from host {}", self.host.name))?;
            let archive = sender.stdout.take().expect("stdout was piped");
            let received = Command::new("tar")
                .args(["-xf", "-", "-C"])
                .arg(&into)
                .stdin(Stdio::from(archive))
                .output()
                .context(|| "running tar".to_string())?;
            let sent = sender
                .wait_with_output()
                .context(|| format!("waiting for host {}", self.host.name))?;
            if !sent.status.success() {
                fail!(
                    "copying {path} from host {} failed ({}): {}",
                    self.host.name,
                    sent.status,
                    String::from_utf8_lossy(&sent.stderr).trim()
                );
            }
            if !received.status.success() {
                fail!(
                    "unpacking {path} from host {} failed: {}",
                    self.host.name,
                    String::from_utf8_lossy(&received.stderr).trim()
                );
            }
        }
        Ok(())
    }
}

/// The script that makes the checkout <name> of <repo> on the host when it
/// has none, set up as a side worktree is, or switches it to <sha> when it
/// has no changes; it prints `worktree=<folder>`. A checkout whose setup
/// fails is removed, so the next build sets it up again rather than finding
/// half of it.
fn ensure_script(host: &Host, repo: &str, name: &str, sha: &str, lane: Lane) -> String {
    let android = match lane {
        Lane::Android => ANDROID_SETUP,
        Lane::Mac => "",
    };
    ENSURE
        .replace("@ROOT@", &root_script(host))
        .replace("@CLONE@", &clone_path(repo))
        .replace("@REPO@", repo)
        .replace("@NAME@", name)
        .replace("@SHA@", &sh_quote(sha))
        .replace("@JOBS@", JOBS_REF)
        .replace("@ANDROID@", android)
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
setup() {
  cd "$wt" || return 1
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

#[cfg(test)]
#[path = "app_remote_tests.rs"]
mod tests;
