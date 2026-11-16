//! `builds remote run`: runs a command in a worktree of a host's clone of a
//! repo, on a commit sent there, and streams its output back.
//!
//! A job sends the commit to the host's clone as the ref
//! `refs/builds/jobs/<id>` (`git push <target>:dev/<repo>`, never to GitHub),
//! claims a worktree on the host (`~/dev/<repo>-worktrees/remote-N`, the first
//! one with no claim, kept between jobs so its build output stays warm),
//! checks the commit out there and runs the command. When the job ends,
//! however it ends, builds deletes the ref and the claim. A stopped `builds
//! remote run` (SIGTERM, SIGINT, SIGHUP) stops the command on the host.
//!
//! On this Mac (host `local`) the clone is `~/dev/<repo>`, which already holds
//! the commit, so nothing is pushed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::checkout::{Checkout, git, git_text};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::host::{self, Host, REMOTE_ROOT, Ssh, VOLUME, choose, clone_path, sh_quote};
use crate::places::Places;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds remote run <repo> <ref> [--host <name>] -- <command> [<args>...]\n       builds remote run <repo> --staged [--host <name>] -- <command> [<args>...]\n       builds remote run <repo> --working [--host <name>] -- <command> [<args>...]";

/// The refs a job's commit is sent as, in the host's clone.
pub const JOBS_REF: &str = "refs/builds/jobs";

/// The exit status builds gives a job a signal stopped.
const STOPPED_EXIT: i32 = 130;

/// Set by the signal handler `stop_on_signals` installs: the person or a
/// parent stopped this command. `cmd_remote` hands it to the job; every other
/// caller gives its job a flag of its own.
static STOPPED: AtomicBool = AtomicBool::new(false);

extern "C" fn note_stop(_: libc::c_int) {
    STOPPED.store(true, Ordering::SeqCst);
}

/// Makes SIGTERM, SIGINT and SIGHUP set `STOPPED`.
pub fn stop_on_signals() -> &'static AtomicBool {
    unsafe {
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(signal, note_stop as *const () as libc::sighandler_t);
        }
    }
    &STOPPED
}

/// What a job runs.
#[derive(Debug, PartialEq, Eq)]
pub enum What {
    Ref(String),
    /// HEAD plus the staged files.
    Staged,
    /// HEAD plus every change in the working tree: staged and unstaged edits,
    /// deletions, and the files git does not track and does not ignore. This
    /// is what a build in the checkout compiles, so `builds cargo` sends it.
    Working,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub repo: String,
    pub what: What,
    pub host: Option<String>,
    pub command: Vec<String>,
}

pub fn parse(args: &[String]) -> Result<Request> {
    let [word, repo, rest @ ..] = args else {
        fail!("{USAGE}");
    };
    if word != "run" {
        fail!("{USAGE}");
    }
    let Some(dashes) = rest.iter().position(|arg| arg == "--") else {
        fail!("{USAGE}");
    };
    let (before, command) = (&rest[..dashes], &rest[dashes + 1..]);
    if command.is_empty() {
        fail!("{USAGE}");
    }
    let mut what = None;
    let mut host = None;
    let mut at = 0;
    while at < before.len() {
        match before[at].as_str() {
            "--host" => {
                let Some(name) = before.get(at + 1) else {
                    fail!("{USAGE}");
                };
                host = Some(name.clone());
                at += 2;
            }
            "--staged" if what.is_none() => {
                what = Some(What::Staged);
                at += 1;
            }
            "--working" if what.is_none() => {
                what = Some(What::Working);
                at += 1;
            }
            flag if flag.starts_with('-') => fail!("{USAGE}"),
            reference if what.is_none() => {
                what = Some(What::Ref(reference.to_string()));
                at += 1;
            }
            _ => fail!("{USAGE}"),
        }
    }
    let Some(what) = what else {
        fail!("{USAGE}");
    };
    Ok(Request {
        repo: repo.clone(),
        what,
        host,
        command: command.to_vec(),
    })
}

/// `git -C <dir>` without the variables a git hook exports for its own
/// repository (`GIT_DIR`, `GIT_INDEX_FILE`, ...): `builds cargo` runs inside
/// the pre-commit hook, and those variables would point git at the hook's
/// index instead of the checkout's files.
fn source_git(dir: &Path) -> Command {
    let mut command = git(dir);
    without_git_variables(&mut command);
    command
}

/// Removes from <command>'s environment the variables a git hook exports for
/// its own repository (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, ...).
fn without_git_variables(command: &mut Command) {
    static LOCAL: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let names = LOCAL.get_or_init(|| crate::checkout::git_local_env_vars().unwrap_or_default());
    for name in names {
        command.env_remove(name);
    }
}

/// The checkout the job's commit comes from: the one the command runs in when
/// it is a checkout of <repo>, else `~/dev/<repo>`.
fn source_of(places: &Places, repo: &str, what: &What) -> Result<PathBuf> {
    let here = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    if let Some(checkout) = Checkout::at(&here)?
        && checkout.repo() == repo
    {
        return Ok(checkout.top);
    }
    if *what == What::Staged || *what == What::Working {
        fail!(
            "--staged and --working send the files of the checkout the command runs in, and this is not a checkout of {repo}"
        );
    }
    let main = places.home.join(clone_path(repo));
    if !main.join(".git").exists() {
        fail!("{} is no clone of {repo}", main.display());
    }
    Ok(main)
}

/// The commit the job runs: <rev>'s, or a new commit holding the staged
/// files or the whole working tree. The temporary commit moves no ref and
/// nothing in the checkout changes: the working tree's files go into an index
/// file of their own, which is deleted afterwards.
fn commit_of(source: &Path, what: &What) -> Result<String> {
    let text = match what {
        What::Working => {
            let git_dir = git_text(
                source_git(source).args(["rev-parse", "--absolute-git-dir"]),
                "find the checkout's git folder",
            )?;
            let index =
                PathBuf::from(git_dir.trim()).join(format!("builds-job-index-{}", job_id()));
            let tree = working_tree(source, &index);
            let _ = std::fs::remove_file(&index);
            commit_tree(source, &tree?, "builds job: the working tree")?
        }
        What::Ref(rev) => git_text(
            source_git(source).args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")]),
            &format!("resolve {rev}"),
        )?,
        What::Staged => {
            let tree = git_text(
                source_git(source).arg("write-tree"),
                "write the staged files' tree",
            )?;
            commit_tree(source, tree.trim(), "builds job: the staged files")?
        }
    };
    Ok(text.trim().to_string())
}

/// A commit of <tree> on top of HEAD that no ref names.
fn commit_tree(source: &Path, tree: &str, message: &str) -> Result<String> {
    git_text(
        source_git(source).args([
            "-c",
            "user.name=builds",
            "-c",
            "user.email=builds@localhost",
            "commit-tree",
            tree,
            "-p",
            "HEAD",
            "-m",
            message,
        ]),
        "commit the files to send",
    )
}

/// The id of a tree holding what is in the working tree of <source>: HEAD's
/// files, with every tracked file as it is on disk (a deleted one gone) and
/// every untracked file git does not ignore. It is built in <index>, a
/// scratch index file in the checkout's git folder (never in the working
/// tree, where it would count as an untracked file), so the real index, which
/// holds what the person staged, is not touched.
fn working_tree(source: &Path, index: &Path) -> Result<String> {
    let in_index = |args: &[&str], what: &str| {
        git_text(
            source_git(source).env("GIT_INDEX_FILE", index).args(args),
            what,
        )
    };
    in_index(&["read-tree", "HEAD"], "read HEAD into a scratch index")?;
    in_index(&["add", "--all", "--", "."], "add the working tree's files")?;
    Ok(in_index(&["write-tree"], "write the working tree's tree")?
        .trim()
        .to_string())
}

/// Where a job's commands run: this Mac, or a host over ssh.
pub struct Transport<'a> {
    pub places: &'a Places,
    pub ssh: &'a Ssh,
    pub host: &'a Host,
}

impl Transport<'_> {
    /// A command that runs <script> with `sh` on the host.
    pub fn command(&self, script: &str) -> Command {
        match &self.host.target {
            Some(target) => self
                .ssh
                .command(target, &format!("sh -c {}", sh_quote(script))),
            None => {
                let mut command = Command::new("sh");
                command.arg("-c").arg(script).env("HOME", &self.places.home);
                // The claim script's git commands name their own worktrees;
                // a GIT_DIR from the hook this runs in would send them to
                // the hook's repository instead.
                without_git_variables(&mut command);
                command
            }
        }
    }

    /// Runs <script> to its end and returns its stdout; a failure names the
    /// host and what the script printed.
    pub fn capture(&self, script: &str, what: &str) -> Result<String> {
        self.ask(script, what)?.map_err(Fail::Message)
    }

    /// Runs <script> to its end. The outer error is this Mac's (the command
    /// would not start); the inner one is the host's: the script failed
    /// there, and the text names the host and what the script printed.
    pub fn ask(&self, script: &str, what: &str) -> Result<std::result::Result<String, String>> {
        let output = self
            .command(script)
            .stdin(Stdio::null())
            .output()
            .context(|| format!("running {what} on host {}", self.host.name))?;
        if !output.status.success() {
            return Ok(Err(format!(
                "{what} failed on host {} ({}): {}",
                self.host.name,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(Ok(String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    /// Sends <sha> to the host's clone as the ref of job <id>; a failure
    /// names the host and what git printed.
    pub fn send(&self, source: &Path, repo: &str, sha: &str, id: &str) -> Result<()> {
        self.push(source, repo, sha, id)?.map_err(Fail::Message)
    }

    /// Sends <sha> to the host's clone as the ref of job <id>. The outer error
    /// is this Mac's; the inner one is the push's, which failed on the way to
    /// the host or there. One send to a host's clone of <repo> runs at a time:
    /// two lanes of one build send the same commit together, and git on the
    /// host fails one of two pushes that receive the same pack at once
    /// ("unable to migrate objects to permanent storage").
    fn push(
        &self,
        source: &Path,
        repo: &str,
        sha: &str,
        id: &str,
    ) -> Result<std::result::Result<(), String>> {
        let Some(target) = &self.host.target else {
            return Ok(Ok(()));
        };
        let folder = self.places.root.join(repo);
        std::fs::create_dir_all(&folder).context(|| format!("creating {}", folder.display()))?;
        let _one_at_a_time = crate::evict::lock_exclusive_creating(
            &folder.join(format!(".send-{}.lock", self.host.name)),
        )?;
        let output = source_git(source)
            .args(["push", "-q", &format!("{target}:{}", clone_path(repo))])
            .arg(format!("{sha}:{JOBS_REF}/{id}"))
            .env("GIT_SSH_COMMAND", self.ssh.git_command())
            .stdin(Stdio::null())
            .output()
            .context(|| "running git push".to_string())?;
        if !output.status.success() {
            return Ok(Err(format!(
                "sending the commit to host {} ({target}) failed: {}",
                self.host.name,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(Ok(()))
    }
}

/// The lines that set what every command builds runs on a host needs, so the
/// host's shell needs no setup: the builds root, and a PATH holding cargo,
/// `builds` itself (`~/.local/bin`), Homebrew and the tools there. A host with
/// a build volume uses it as its root, as this Mac does; a host without one
/// keeps the root in its home. `BUILDS_VOLUME` stands in for
/// `/Volumes/Builds` so tests can name a volume that is there or is not.
/// On this Mac builds already runs with its root and PATH.
pub fn root_script(host: &Host) -> String {
    match host.target {
        Some(_) => format!(
            r#"if [ -d "${{BUILDS_VOLUME:-{VOLUME}}}" ]; then BUILDS_ROOT="${{BUILDS_VOLUME:-{VOLUME}}}"; else BUILDS_ROOT="$HOME/{REMOTE_ROOT}"; fi
export BUILDS_ROOT
PATH="$HOME/.cargo/bin:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH
"#
        ),
        None => String::new(),
    }
}

/// Finds the repo's clone and claims the first free worktree for job <id>,
/// with <sha> checked out in it, then sets the worktree up as `builds new`
/// sets up a side worktree: the repo's `scripts/hooks/post-checkout` runs in
/// it as git runs it for a new worktree (build folders linked into the
/// builds root, shared files linked from the main checkout). Prints
/// `worktree=<path>`.
fn claim_script(host: &Host, repo: &str, sha: &str, id: &str) -> String {
    format!(
        r#"{root}set -e
clone="$HOME/{clone}"
if [ ! -e "$clone/.git" ]; then echo "no clone of {repo} at $clone" >&2; exit 90; fi
wts="$HOME/dev/{repo}-worktrees"
mkdir -p "$wts"
n=1
while ! mkdir "$wts/remote-$n.job" 2>/dev/null; do n=$((n+1)); done
wt="$wts/remote-$n"
echo {id} > "$wts/remote-$n.job/id"
if [ -e "$wt/.git" ]; then
  git -C "$wt" checkout -q --force --detach {sha} && git -C "$wt" clean -fdq
else
  git -C "$clone" worktree add -q --detach "$wt" {sha}
fi || {{ rm -rf "$wts/remote-$n.job"; exit 91; }}
if [ -x "$wt/scripts/hooks/post-checkout" ]; then
  if ! said=$(cd "$wt" && "$wt/scripts/hooks/post-checkout" 0000000000000000000000000000000000000000 {sha} 1 2>&1); then
    echo "post-checkout failed in $wt: $said" >&2
    rm -rf "$wts/remote-$n.job"
    exit 93
  fi
fi
echo "worktree=$wt""#,
        root = root_script(host),
        clone = clone_path(repo),
        repo = repo,
        sha = sh_quote(sha),
        id = sh_quote(id),
    )
}

/// Runs <command> in <worktree> as a process group of its own, whose leader's
/// pid goes in the claim folder for `stop_script`.
pub fn run_script(host: &Host, worktree: &str, command: &[String]) -> String {
    let words: Vec<String> = command.iter().map(|word| sh_quote(word)).collect();
    format!(
        r#"{root}# ssh's shell starts with macOS's default of 256 open files, which cargo's
# tests (many stores open at once) exceed; a login shell here has more.
ulimit -n 10240 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null
set -m
cd {wt} || exit 92
( exec {words} ) &
pid=$!
echo $pid > {wt}.job/pid
wait $pid 2>/dev/null"#,
        root = root_script(host),
        wt = sh_quote(worktree),
        words = words.join(" "),
    )
}

fn stop_script(worktree: &str) -> String {
    format!(
        r#"pid=$(cat {wt}.job/pid) && kill -TERM -- "-$pid""#,
        wt = sh_quote(worktree)
    )
}

/// Releases the worktree's claim and deletes the job's ref.
pub fn release_script(repo: &str, worktree: Option<&str>, id: &str) -> String {
    let claim = match worktree {
        Some(worktree) => format!("rm -rf {}.job\n", sh_quote(worktree)),
        None => String::new(),
    };
    format!(
        "{claim}git -C \"$HOME/{clone}\" update-ref -d {JOBS_REF}/{id}",
        clone = clone_path(repo),
        id = id,
    )
}

/// Runs the job's command on the host, passing its output through, and
/// returns its exit status. A stop signal stops the command on the host. With
/// <time_locks> (a cargo command), its stderr passes through builds, which
/// also returns how long cargo waited for its file locks (lock_wait.rs).
pub fn run_command(
    transport: &Transport,
    stop: &AtomicBool,
    worktree: &str,
    command: &[String],
    time_locks: bool,
) -> Result<(i32, Option<Duration>)> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let mut child = transport
        .command(&run_script(transport.host, worktree, command))
        .stdin(Stdio::null())
        .stderr(if time_locks {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        // Its own process group: a Ctrl-C reaches builds, which stops the
        // command on the host itself.
        .process_group(0)
        .spawn()
        .context(|| format!("running the command on host {}", transport.host.name))?;
    let relay = child
        .stderr
        .take()
        .map(|stderr| crate::lock_wait::relay(stderr, std::io::stderr()));
    let waited = || relay.map(|relay| relay.join().expect("the relay does not panic"));
    let mut stopped = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context(|| "waiting for the command".to_string())?
        {
            break status;
        }
        if stop.load(Ordering::SeqCst) && !stopped {
            stopped = true;
            if let Err(error) = transport.capture(&stop_script(worktree), "stopping the command") {
                note(error.text());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let waited = waited();
    if stopped {
        return Ok((STOPPED_EXIT, waited));
    }
    Ok((
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
        waited,
    ))
}

/// The worktree path the claim script printed.
pub fn worktree_in(output: &str) -> Result<String> {
    match output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("worktree="))
    {
        Some(path) if !path.is_empty() => Ok(path.to_string()),
        _ => fail!("the host did not say which worktree it claimed: {output:?}"),
    }
}

/// A name for one job's ref on a host, unique among the jobs of all processes.
pub fn job_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_nanos()
    )
}

/// A job whose commit is on its host and whose worktree there is claimed:
/// what is left is to run its command (`Job::run`).
pub struct Job<'a> {
    places: &'a Places,
    ssh: &'a Ssh,
    host: Host,
    repo: String,
    command: Vec<String>,
    worktree: String,
    id: String,
}

/// How the start of a job went.
pub enum Start<'a> {
    Started(Job<'a>),
    /// The host would not take the job: sending its commit or claiming a
    /// worktree failed there, before its command ran. Holds what the host
    /// said; `start_job` has recorded the host as refusing jobs.
    Refused(String),
}

/// Sends the commit to <host> and claims a worktree there. A failure on this
/// Mac (making the commit, starting git or ssh) is an error; a failure of the
/// host is `Start::Refused`, except on this Mac, which never refuses its own
/// jobs. Whatever the job left on the host when it did not start is removed.
pub fn start_job<'a>(
    places: &'a Places,
    ssh: &'a Ssh,
    host: &Host,
    source: &Path,
    request: &Request,
) -> Result<Start<'a>> {
    let sha = commit_of(source, &request.what)?;
    let id = job_id();
    let transport = Transport { places, ssh, host };
    // The outer error is this Mac's; the inner one is the host's.
    let started = (|| -> Result<std::result::Result<String, String>> {
        if let Err(said) = transport.push(source, &request.repo, &sha, &id)? {
            return Ok(Err(said));
        }
        let claimed = match transport.ask(
            &claim_script(host, &request.repo, &sha, &id),
            "claiming a worktree",
        )? {
            Ok(claimed) => claimed,
            Err(said) => return Ok(Err(said)),
        };
        Ok(worktree_in(&claimed).map_err(|error| format!("host {}: {}", host.name, error.text())))
    })();
    // Whatever the job left on the host when it did not start.
    let release = || {
        if let Err(release) = transport.capture(
            &release_script(&request.repo, None, &id),
            "removing the job's ref and claim",
        ) {
            note(release.text());
        }
    };
    let said = match started {
        Ok(Ok(worktree)) => {
            return Ok(Start::Started(Job {
                places,
                ssh,
                host: host.clone(),
                repo: request.repo.clone(),
                command: request.command.clone(),
                worktree,
                id,
            }));
        }
        Ok(Err(said)) => said,
        Err(error) => {
            release();
            return Err(error);
        }
    };
    release();
    if host.target.is_none() {
        fail!("{said}");
    }
    if let Err(error) = host::record_refusal(places, host::Refused::Jobs, &host.name, &said) {
        note(error.text());
    }
    Ok(Start::Refused(said))
}

impl Job<'_> {
    /// The name of the host the job runs on.
    pub fn host_name(&self) -> &str {
        &self.host.name
    }

    /// Runs the job's command on its host, passing its output through,
    /// removes the job's ref and claim, and returns the command's exit status.
    pub fn run(self, stop: &AtomicBool) -> Result<i32> {
        Ok(self.run_with(stop, false)?.0)
    }

    /// `run` for a cargo command: also returns how long cargo waited for its
    /// file locks on the host, read from the messages it passes through.
    pub fn run_timing_locks(self, stop: &AtomicBool) -> Result<(i32, Duration)> {
        let (code, waited) = self.run_with(stop, true)?;
        Ok((code, waited.expect("the messages were read")))
    }

    fn run_with(self, stop: &AtomicBool, time_locks: bool) -> Result<(i32, Option<Duration>)> {
        let transport = Transport {
            places: self.places,
            ssh: self.ssh,
            host: &self.host,
        };
        let ran = run_command(&transport, stop, &self.worktree, &self.command, time_locks);
        let released = transport.capture(
            &release_script(&self.repo, Some(&self.worktree), &self.id),
            "removing the job's ref and claim",
        );
        let ran = match ran {
            Ok(ran) => ran,
            Err(error) => {
                if let Err(release) = released {
                    note(release.text());
                }
                return Err(error);
            }
        };
        if let Err(error) = released {
            if ran.0 == 0 {
                return Err(error);
            }
            note(error.text());
        }
        Ok(ran)
    }
}

/// Sends the commit to <host>, runs the command there and cleans up; the
/// command's exit status. A host that refuses the job fails it, saying what
/// the host said.
pub fn run_job(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    source: &Path,
    request: &Request,
    stop: &AtomicBool,
) -> Result<i32> {
    match start_job(places, ssh, host, source, request)? {
        Start::Started(job) => job.run(stop),
        Start::Refused(said) => fail!("{said}"),
    }
}

/// Picks the host: the one named, or the least loaded.
pub fn pick_host(
    places: &Places,
    ssh: &Ssh,
    floor_kib: u64,
    named: Option<&str>,
) -> Result<(Host, String)> {
    let hosts = host::all(places)?;
    match named {
        Some(name) => {
            let Some(found) = hosts.into_iter().find(|host| host.name == name) else {
                fail!("no host named {name} (`builds host list`)");
            };
            // A named host that does not answer fails the job; nothing falls
            // back to another.
            match host::probe(places, ssh, &found) {
                Ok(_) => Ok((found, format!("host {name}, named with --host"))),
                Err(error) => fail!("host {name} is not reachable: {error}"),
            }
        }
        None => choose(&host::probe_all(places, ssh, hosts), floor_kib),
    }
}

/// `builds remote step <host> <worktree> -- <command>...`: runs the command in
/// <worktree> on the host, which is a folder there that already holds the
/// files to run on (an app checkout, set up by `app_remote`), and passes its
/// output back. A stop signal stops the command on the host. `builds
/// app-build` runs each step of a lane on a host through this, so its time
/// limit and its takeover stop the step on the host the way they stop one
/// here.
fn cmd_step(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    const STEP_USAGE: &str = "usage: builds remote step <host> <worktree> -- <command> [<args>...]";
    let [_, name, worktree, dashes, command @ ..] = args else {
        fail!("{STEP_USAGE}");
    };
    if dashes != "--" || command.is_empty() {
        fail!("{STEP_USAGE}");
    }
    let Some(host) = host::all(&tool.places)?
        .into_iter()
        .find(|host| host.name == *name)
    else {
        fail!("no host named {name} (`builds host list`)");
    };
    let transport = Transport {
        places: &tool.places,
        ssh,
        host: &host,
    };
    match run_command(&transport, stop_on_signals(), worktree, command, false)?.0 {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

pub fn cmd_remote(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    if args.first().is_some_and(|word| word == "step") {
        return cmd_step(tool, ssh, args);
    }
    let request = parse(args)?;
    let source = source_of(&tool.places, &request.repo, &request.what)?;
    let (host, why) = pick_host(
        &tool.places,
        ssh,
        tool.rules.floor_kib,
        request.host.as_deref(),
    )?;
    note(why);
    match run_job(
        &tool.places,
        ssh,
        &host,
        &source,
        &request,
        stop_on_signals(),
    )? {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
