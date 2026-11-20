//! `builds remote run`: runs a command in a worktree of a host's clone of a
//! repo, on a commit sent there, and streams its output back.
//!
//! A job sends the commit to the host's clone as the ref
//! `refs/builds/jobs/<id>` (`git push <target>:dev/<repo>`, never to GitHub),
//! claims the worktree on the host that belongs to the checkout the job comes
//! from (`~/dev/<repo>-worktrees/remote-<checkout's folder name>`, or
//! `remote-<name>.<n>` while another job from that checkout holds it),
//! checks the commit out there and runs the command. builds keeps the
//! worktree between jobs, so its files and build output stay as that
//! checkout's last job left them, until the checkout is gone and `builds
//! prune` removes it (remote_worktrees.rs). When the job ends, however it
//! ends, builds deletes the ref and the claim; when the `builds` process
//! itself is killed first, the next job takes the claim over (job_claim.rs).
//! A stopped `builds remote run` (SIGTERM, SIGINT, SIGHUP) stops the command
//! on the host, and a `builds` process that ends any other way, SIGKILL
//! included, stops it too: the command lives only while its caller holds the
//! command's stdin open (`run_script`). The command's run ends when the
//! command does, even when it leaves processes behind.
//!
//! On this Mac (host `local`) the clone is `~/dev/<repo>`, which already holds
//! the commit, so nothing is pushed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use crate::awake::KeepAwake;
use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::host::{self, Host, LINUX_ROOT, REMOTE_ROOT, Ssh, VOLUME, choose, clone_path, sh_quote};
use crate::host_platform::Os;
use crate::job_claim::Holders;
use crate::places::Places;
use crate::setup::Prepare;
use crate::text::note;
use crate::tool::Tool;
use boards::{git, git_text};

const USAGE: &str = "usage: builds remote run <repo> <ref> [--host <name>] -- <command> [<args>...]\n       builds remote run <repo> --staged [--host <name>] -- <command> [<args>...]\n       builds remote run <repo> --working [--host <name>] -- <command> [<args>...]";

/// The refs a job's commit is sent as, in the host's clone.
pub const JOBS_REF: &str = "refs/builds/jobs";

/// The prefix of the name of every worktree a job runs in (`pinned_name`).
pub const JOB_WORKTREE_PREFIX: &str = "remote-";

/// The file in a worktree's claim folder (`<worktree>.job`) where the run
/// script records the command that runs there: `<group> <run>`, the id of
/// the command's process group and the run's id (`job_id`). The script
/// removes it once the command has ended and nothing is left in its group,
/// and each host's scan (host_scan.rs) reads it to list the commands that
/// jobs run there.
pub const RUN_FILE: &str = "run";

/// The exit status builds gives a job a signal stopped.
const STOPPED_EXIT: i32 = 130;

/// The exit status of ssh when its connection fails, which is also ssh's
/// status when the command it ran exits 255.
pub const SSH_FAILED: i32 = 255;

/// Set by the signal handler `stop_on_signals` installs: the person or a
/// parent stopped this command.
static STOPPED: AtomicBool = AtomicBool::new(false);

/// How many commands one process can run on hosts at once (`LIFELINES`).
const LIFELINE_SLOTS: usize = 16;

/// A slot of `LIFELINES` that no `run_command` holds.
const FREE: i32 = -1;

/// A slot of `LIFELINES` that a `run_command` holds with no open descriptor in
/// it: before the command starts, or once its lifeline is cut.
const CLAIMED: i32 = -2;

/// The write ends of the stdin pipes of the commands `run_command` runs: each
/// command's lifeline (`run_script`), one per slot, else `FREE` or `CLAIMED`.
/// A fixed array of atomics, so the signal handler can cut every lifeline.
/// Whoever swaps a descriptor out of its slot closes it (`cut`), so each is
/// closed once.
static LIFELINES: [AtomicI32; LIFELINE_SLOTS] = [const { AtomicI32::new(FREE) }; LIFELINE_SLOTS];

/// Closes the lifeline <slot> holds open, if it holds one, which stops that
/// command on its host. The signal handler calls it, so it makes only
/// async-signal-safe calls.
fn cut(slot: &AtomicI32) {
    let fd = slot.load(Ordering::SeqCst);
    if fd >= 0
        && slot
            .compare_exchange(fd, CLAIMED, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        // SAFETY: the exchange made this the only owner of the descriptor.
        unsafe { libc::close(fd) };
    }
}

/// One `run_command`'s slot in `LIFELINES`, from before its command starts
/// until the call ends. Dropping it cuts the lifeline and frees the slot.
struct Lifeline(&'static AtomicI32);

impl Lifeline {
    /// Claims a free slot, or fails when every slot is held.
    fn claim() -> Result<Lifeline> {
        for slot in &LIFELINES {
            if slot
                .compare_exchange(FREE, CLAIMED, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Ok(Lifeline(slot));
            }
        }
        fail!(
            "this process already runs {LIFELINE_SLOTS} commands on hosts, the most builds can stop at once"
        )
    }

    /// Puts the command's open lifeline <fd> in the slot. A stop that came
    /// before found nothing here to cut, so this cuts it.
    fn hold(&self, fd: std::os::unix::io::RawFd) {
        self.0.store(fd, Ordering::SeqCst);
        if STOPPED.load(Ordering::SeqCst) {
            cut(self.0);
        }
    }
}

impl Drop for Lifeline {
    fn drop(&mut self) {
        cut(self.0);
        self.0.store(FREE, Ordering::SeqCst);
    }
}

extern "C" fn note_stop(_: libc::c_int) {
    STOPPED.store(true, Ordering::SeqCst);
    for slot in &LIFELINES {
        cut(slot);
    }
}

/// Makes SIGTERM, SIGINT and SIGHUP set `STOPPED` and cut the lifeline of
/// every command `run_command` runs. A caller that does work on the host before the
/// command starts (the send, the claim) calls it first, so a stop during
/// that work stops the command as soon as it starts.
pub fn stop_on_signals() {
    unsafe {
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(signal, note_stop as *const () as libc::sighandler_t);
        }
    }
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
    /// Whether the host's setup of the worktree prepares it (the repo's
    /// `prepare` programs and tools, setup.rs): `Prepare::No` for a job a
    /// pre-commit hook check started.
    pub prepare: Prepare,
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
        prepare: Prepare::Yes,
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

/// Runs <sender> with its stdout piped into <receiver>, and fails, naming
/// <what> and the side that failed with what it printed, unless both succeed.
pub fn pipe(mut sender: Command, mut receiver: Command, what: &str) -> Result<()> {
    let mut send = sender
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(|| format!("starting the copy of {what}"))?;
    let out = send.stdout.take().expect("stdout was piped");
    let received = receiver
        .stdin(Stdio::from(out))
        .stderr(Stdio::piped())
        .output()
        .context(|| format!("starting to receive {what}"))?;
    let sent = send
        .wait_with_output()
        .context(|| format!("waiting for the copy of {what}"))?;
    if !sent.status.success() {
        fail!(
            "copying {what} failed ({}): {}",
            sent.status,
            String::from_utf8_lossy(&sent.stderr).trim()
        );
    }
    if !received.status.success() {
        fail!(
            "receiving {what} failed ({}): {}",
            received.status,
            String::from_utf8_lossy(&received.stderr).trim()
        );
    }
    Ok(())
}

/// Unpacks the tar archive <sender> writes on its stdout into <into> on this
/// Mac (`pipe`).
pub fn unpack(sender: Command, into: &Path, what: &str) -> Result<()> {
    let mut tar = Command::new("tar");
    tar.args(["-xf", "-", "-C"]).arg(into);
    pipe(sender, tar, what)
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
    Ok(git_text(
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
    )?)
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
        let mut command = host::command_at(self.ssh, self.host, script, host::Terminal::None);
        if self.host.target.is_none() {
            command.env("HOME", &self.places.home);
            // The claim script's git commands name their own worktrees; a
            // GIT_DIR from the hook this runs in would send them to the
            // hook's repository instead.
            without_git_variables(&mut command);
        }
        command
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
/// `builds` itself (`~/.local/bin`), Homebrew and the tools there. A Mac with
/// a build volume uses it as its root, as this Mac does; a Mac without one
/// keeps the root in its home, in a folder Spotlight skips; a Linux host
/// keeps it in its home (`LINUX_ROOT`), where the person can mount a disk of
/// its own. `BUILDS_VOLUME` stands in for `/Volumes/Builds` so tests can name
/// a volume that is there or is not. On this Mac builds already runs with its
/// root and PATH.
pub fn root_script(host: &Host) -> String {
    match host.target {
        Some(_) => format!(
            r#"case "$(uname -s)" in
Darwin) if [ -d "${{BUILDS_VOLUME:-{VOLUME}}}" ]; then BUILDS_ROOT="${{BUILDS_VOLUME:-{VOLUME}}}"; else BUILDS_ROOT="$HOME/{REMOTE_ROOT}"; fi ;;
*) BUILDS_ROOT="$HOME/{LINUX_ROOT}" ;;
esac
export BUILDS_ROOT
PATH="$HOME/.cargo/bin:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH
"#
        ),
        None => String::new(),
    }
}

/// The name of the worktree on a host that jobs from the checkout <source>
/// use: `remote-<the checkout's folder name>`, so each checkout here builds
/// in its own worktree there, whose files and build folders stay as its last
/// job left them. A second job from the same checkout while the first runs
/// takes `remote-<name>.2`, and so on.
pub fn pinned_name(source: &Path) -> Result<String> {
    match source.file_name().and_then(|name| name.to_str()) {
        Some(name) if !name.is_empty() => Ok(format!("{JOB_WORKTREE_PREFIX}{name}")),
        _ => fail!(
            "{} has no folder name to name its worktree on a host",
            source.display()
        ),
    }
}

/// Finds the repo's clone and claims worktree <pinned> for job <id>, or the
/// first free `<pinned>.<n>` while another job holds it, with <sha> checked
/// out in it. A claim whose process <holders> found gone holds nothing: the
/// script removes it and claims that worktree (job_claim.rs). The script
/// then sets the worktree up as `builds new` sets up a side worktree: the
/// host's `builds setup` links the build folders into the host's builds
/// root, and then the repo's `scripts/hooks/post-checkout` runs in it as git
/// runs it for a new worktree (shared files linked from the main
/// checkout). Both run for every job, so a worktree that an earlier job left
/// without its links gets them. builds runs setup itself because a repo with
/// no post-checkout, such as coven, would otherwise build into a real
/// `target` folder in the worktree, outside the build volume and every cap.
/// Prints `worktree=<path>`.
fn claim_script(
    host: &Host,
    repo: &str,
    pinned: &str,
    sha: &str,
    id: &str,
    holders: &Holders,
    prepare: Prepare,
) -> String {
    format!(
        r#"{root}{claims}set -e
clone="$HOME/{clone}"
if [ ! -e "$clone/.git" ]; then echo "no clone of {repo} at $clone" >&2; exit 90; fi
wts="$HOME/dev/{repo}-worktrees"
mkdir -p "$wts"
pinned="$wts"/{pinned}
wt="$pinned"
n=1
until mkdir "$wt.job" 2>/dev/null; do
  gone=$(claim_gone "$wt.job") && release_gone "$wt.job" "$gone" && continue
  n=$((n+1))
  wt="$pinned.$n"
done
echo {id} > "$wt.job/id"
if [ -e "$wt/.git" ]; then
  git -C "$wt" checkout -q --force --detach {sha} && git -C "$wt" clean -fdq
else
  git -C "$clone" worktree add -q --detach "$wt" {sha}
fi || {{ rm -rf "$wt.job"; exit 91; }}
if ! said=$("$HOME/.local/bin/builds" setup {setup_flags}"$wt" 2>&1); then
  echo "builds setup failed in $wt: $said" >&2
  rm -rf "$wt.job"
  exit 94
fi
if [ -x "$wt/scripts/hooks/post-checkout" ]; then
  if ! said=$(cd "$wt" && "$wt/scripts/hooks/post-checkout" 0000000000000000000000000000000000000000 {sha} 1 2>&1); then
    echo "post-checkout failed in $wt: $said" >&2
    rm -rf "$wt.job"
    exit 93
  fi
fi
echo "worktree=$wt""#,
        root = root_script(host),
        claims = holders.script(),
        clone = clone_path(repo),
        repo = repo,
        pinned = sh_quote(pinned),
        sha = sh_quote(sha),
        id = sh_quote(id),
        setup_flags = match prepare {
            Prepare::Yes => "",
            Prepare::No => "--no-prepare ",
        },
    )
}

/// Runs <command> in <worktree> as a process group of its own, and exits with
/// the command's exit status as soon as the command ends.
///
/// ssh returns only once nothing on the host holds its output open, and a
/// command can leave processes behind that do (a build daemon, a simulator's
/// helper), some of them in process groups of their own. So the command
/// writes to two FIFOs in the claim folder, never to ssh's output, and a relay
/// for each passes what comes through on to ssh's stdout or stderr. When the
/// command ends, the script stops what is left in its process group and
/// writes a line that marks the end of run <run> to each FIFO, after
/// everything the command wrote. Each relay passes on what came before the
/// marker and exits. A process the command left behind may still hold a
/// FIFO, but nothing holds ssh's output. When the script cannot stop what is
/// left in the group, it says so and fails a command that passed.
///
/// The command lives no longer than its caller wants it to. The script's
/// stdin is the pipe `run_command` holds the only write end of (through ssh on
/// a host), and nothing ever writes to it, so stdin reaches its end only when
/// the caller closes it on a stop signal, when the caller's `builds` process
/// ends, however it ends, SIGKILL included, or when the connection to the
/// host closes. A watcher reads stdin to its end, then stops the command's
/// process group. Once the command has ended, the script stops the watcher.
///
/// While the command runs, the script records its process group and <run>
/// in the claim folder's `RUN_FILE`. It writes the record whole, through a
/// file it then renames, so a reader never sees half of it.
pub fn run_script(host: &Host, worktree: &str, command: &[String], run: &str) -> String {
    let words: Vec<String> = command.iter().map(|word| sh_quote(word)).collect();
    format!(
        r#"{root}# ssh's shell starts with macOS's default of 256 open files, which cargo's
# tests (many stores open at once) exceed; a login shell here has more.
ulimit -n 10240 2>/dev/null || ulimit -n "$(ulimit -Hn)" 2>/dev/null
# The command and the watcher each run in a process group of their own, whose
# id is the pid of its first process. Linux has setsid, which gives one;
# macOS has no setsid, and its sh gives each background job a group of its own
# under job control (set -m). Linux's sh (dash) turns job control off with no
# terminal, which ssh gives none. The script turns job control off once it
# has started the watcher. dash's kill takes no `--`, so each kill below
# names the group as `-<pid>` alone.
if command -v setsid >/dev/null 2>&1; then own_group=setsid; else set -m; own_group=; fi
# The caller's pipe, kept on 5: a shell may give a background job /dev/null
# as its stdin.
exec 5<&0 </dev/null
cd {wt} || exit 92
job={wt}.job
rm -f "$job/out" "$job/err"
mkfifo "$job/out" "$job/err" || exit 95
end={end}
relay='{{ at = index($0, end); if (at) {{ if (at > 1) print substr($0, 1, at - 1); fflush(); exit }} print; fflush() }}'
LC_ALL=C awk -v end="$end" "$relay" < "$job/out" &
out=$!
LC_ALL=C awk -v end="$end" "$relay" < "$job/err" >&2 &
err=$!
exec 3>"$job/out" 4>"$job/err"
( exec $own_group {words} < /dev/null >&3 2>&4 3>&- 4>&- 5<&- ) &
pid=$!
( exec $own_group sh -c 'cat >/dev/null; kill -TERM "-$1"' sh "$pid" ) <&5 >/dev/null 2>&1 3>&- 4>&- 5<&- &
watcher=$!
# Job control is off again: the groups are made, and sh prints a status line on
# stderr for each background job that ends while it is on.
[ -n "$own_group" ] || set +m
exec 5<&-
if ! said=$( {{ echo "$pid" {run} > "$job/{run_file}.new" && mv "$job/{run_file}.new" "$job/{run_file}"; }} 2>&1 ); then
  echo "builds could not record the command's process group $pid in $job on host {host}, so builds health will not list it: $said" >&2
fi
wait $pid 2>/dev/null
status=$?
kill -TERM "-$watcher" 2>/dev/null
# Waiting for it here, with stderr silenced, is where sh reports its signal.
wait $watcher 2>/dev/null
if kill -0 "-$pid" 2>/dev/null; then
  if ! said=$(kill -TERM "-$pid" 2>&1); then
    echo "builds could not stop the processes the command left in its process group $pid on host {host}: $said" >&2
    [ $status -eq 0 ] && status=96
  fi
fi
rm -f "$job/{run_file}" "$job/{run_file}.new"
echo "$end" >&3
echo "$end" >&4
exec 3>&- 4>&-
wait $out $err
rm -f "$job/out" "$job/err"
exit $status"#,
        root = root_script(host),
        wt = sh_quote(worktree),
        words = words.join(" "),
        end = sh_quote(&format!("builds-end-of-run-{run}")),
        run = sh_quote(run),
        run_file = RUN_FILE,
        host = host.name,
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
/// returns its exit status. A stop signal stops the command on the host, and
/// so does the end of this process, by any signal: both close the command's
/// stdin (`run_script`). When <cargo> is given (the command is cargo), builds
/// reads the command's stdout and stderr as they pass through, gives each
/// line to that watch, and also returns how long cargo waited for its file
/// locks (lock_wait.rs). When ssh to a host exits 255, builds says on stderr
/// which sleeps this Mac's power log and the host's record while the command
/// ran (sleeps.rs).
pub fn run_command(
    transport: &Transport,
    worktree: &str,
    command: &[String],
    cargo: Option<&crate::cargo_run_outcome::Watch>,
) -> Result<(i32, Option<Duration>)> {
    use std::os::unix::io::IntoRawFd;
    use std::os::unix::process::CommandExt;
    stop_on_signals();
    let lifeline = Lifeline::claim()?;
    let run = job_id();
    let started = crate::app_timing::now();
    let mut child = transport
        .command(&run_script(transport.host, worktree, command, &run))
        // The command's lifeline (`run_script`): this process holds the
        // pipe's only write end until the command has ended or a stop signal
        // cuts it (`Lifeline`), and the kernel closes it when this process
        // ends.
        .stdin(Stdio::piped())
        .stdout(if cargo.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stderr(if cargo.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        // Its own process group: a Ctrl-C reaches builds, which stops the
        // command on the host itself.
        .process_group(0)
        .spawn()
        .context(|| format!("running the command on host {}", transport.host.name))?;
    lifeline.hold(child.stdin.take().expect("stdin was piped").into_raw_fd());
    let relay = child.stderr.take().zip(cargo).map(|(stderr, watch)| {
        crate::lock_wait::relay(
            stderr,
            crate::cargo_run_outcome::Tee::new(std::io::stderr(), watch.clone()),
        )
    });
    let out = child
        .stdout
        .take()
        .zip(cargo)
        .map(|(stdout, watch)| relay_stdout(stdout, watch.clone()));
    let waited = || {
        if let Some(out) = out {
            out.join().expect("the relay does not panic");
        }
        relay.map(|relay| relay.join().expect("the relay does not panic"))
    };
    let status = child.wait();
    let ended = crate::app_timing::now();
    drop(lifeline);
    let status = status.context(|| "waiting for the command".to_string())?;
    let waited = waited();
    if STOPPED.load(Ordering::SeqCst) {
        return Ok((STOPPED_EXIT, waited));
    }
    let code = crate::fail::exit_code(status);
    if code == SSH_FAILED && transport.host.target.is_some() {
        note(crate::sleeps::why_dropped(
            transport.ssh,
            transport.host,
            started,
            ended,
        ));
    }
    Ok((code, waited))
}

/// Passes the host's stdout <from> on to this process's stdout, giving each
/// line to <watch>, until the host closes it. A read that fails ends the
/// relay, and a write that fails leaves the rest unprinted while <watch>
/// still reads it; the relay says each.
fn relay_stdout(
    mut from: impl std::io::Read + Send + 'static,
    watch: crate::cargo_run_outcome::Watch,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut to = crate::cargo_run_outcome::Tee::new(std::io::stdout(), watch);
        let mut printing = true;
        let mut chunk = [0u8; 8192];
        loop {
            let read = match from.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    note(format!("reading the command's output stopped: {error}"));
                    break;
                }
            };
            if let Err(error) = to.write_all_watched(&chunk[..read], printing) {
                note(format!("printing the command's output stopped: {error}"));
                printing = false;
            }
        }
    })
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
    format!("{}-{}", std::process::id(), crate::job_claim::now_nanos())
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
    /// Keeps this Mac awake from the job's send until its command has run
    /// and its claim is released (awake.rs).
    _awake: KeepAwake,
}

/// How the start of a job went.
pub enum Start<'a> {
    Started(Box<Job<'a>>),
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
    let awake = KeepAwake::for_host(host, &format!("a job of {}", request.repo))?;
    let sha = commit_of(source, &request.what)?;
    let pinned = pinned_name(source)?;
    let id = job_id();
    let transport = Transport { places, ssh, host };
    // The outer error is this Mac's; the inner one is the host's.
    let started = (|| -> Result<std::result::Result<String, String>> {
        if let Err(said) = transport.push(source, &request.repo, &sha, &id)? {
            return Ok(Err(said));
        }
        let claimed = match transport.ask(
            &claim_script(
                host,
                &request.repo,
                &pinned,
                &sha,
                &id,
                &Holders::read()?,
                request.prepare,
            ),
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
            return Ok(Start::Started(Box::new(Job {
                places,
                ssh,
                host: host.clone(),
                repo: request.repo.clone(),
                command: request.command.clone(),
                worktree,
                id,
                _awake: awake,
            })));
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
    pub fn run(self) -> Result<i32> {
        Ok(self.run_with(None)?.0)
    }

    /// Runs a cargo command like `run` and also returns how long cargo waited
    /// for its file locks on the host, which builds reads from cargo's
    /// messages as they pass through.
    pub fn run_timing_locks(
        self,
        watch: &crate::cargo_run_outcome::Watch,
    ) -> Result<(i32, Duration)> {
        let (code, waited) = self.run_with(Some(watch))?;
        Ok((
            code,
            waited.expect("run_command relays the command's stderr when it times locks"),
        ))
    }

    fn run_with(
        self,
        cargo: Option<&crate::cargo_run_outcome::Watch>,
    ) -> Result<(i32, Option<Duration>)> {
        let transport = Transport {
            places: self.places,
            ssh: self.ssh,
            host: &self.host,
        };
        let ran = run_command(&transport, &self.worktree, &self.command, cargo);
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
) -> Result<i32> {
    match start_job(places, ssh, host, source, request)? {
        Start::Started(job) => job.run(),
        Start::Refused(said) => fail!("{said}"),
    }
}

/// The host a job runs on, the OS it runs, and the line that says why
/// `pick_host` picked it.
pub struct Picked {
    pub host: Host,
    pub os: Os,
    pub why: String,
}

/// Picks the host: the one named, or the least loaded, which runs this
/// machine's OS (`host::choose`).
pub fn pick_host(
    places: &Places,
    ssh: &Ssh,
    floor_kib: u64,
    named: Option<&str>,
) -> Result<Picked> {
    let hosts = host::all(places)?;
    match named {
        Some(name) => {
            let Some(found) = hosts.into_iter().find(|host| host.name == name) else {
                fail!("no host named {name} (`builds host list`)");
            };
            // A named host that does not answer fails the job; nothing falls
            // back to another.
            match host::probe(places, ssh, &found) {
                Ok(probe) => Ok(Picked {
                    host: found,
                    os: probe.platform.os,
                    why: format!("host {name}, named with --host"),
                }),
                Err(error) => fail!("host {name} is not reachable: {error}"),
            }
        }
        None => {
            let (host, why) = choose(&host::probe_all(places, ssh, hosts), floor_kib)?;
            Ok(Picked {
                host,
                os: Os::here(),
                why,
            })
        }
    }
}

/// Runs <request> on <host>, which runs <os>, as `run_job` does. Only one
/// process at a time opens windows on a Mac, and builds cannot tell whether a
/// command opens any, so a job on a Mac holds the host's window lock
/// (window_lock.rs) from before it sends the commit until it ends, and waits
/// for the lock while an app build's step or another job holds it. A Linux
/// host has no display, so a job there takes no lock. The wait comes before
/// `stop_on_signals`, so a stop signal ends a job that waits at once.
pub fn run_job_on(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    os: Os,
    source: &Path,
    request: &Request,
) -> Result<i32> {
    let _window = match os {
        Os::Mac => {
            let what = format!("{}'s job `{}`", request.repo, request.command.join(" "));
            let taken =
                crate::window_lock::take(places, &host.name, &what, &|words| note(words), &|| {
                    false
                })?;
            let Some(window) = taken else {
                unreachable!("nothing ends the wait for the window lock but the lock");
            };
            Some(window)
        }
        Os::Linux => None,
    };
    // A stop during the send or the claim stops the command once it starts.
    stop_on_signals();
    run_job(places, ssh, host, source, request)
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
    match run_command(&transport, worktree, command, None)?.0 {
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
    let picked = pick_host(
        &tool.places,
        ssh,
        tool.rules.floor_kib,
        request.host.as_deref(),
    )?;
    note(picked.why);
    match run_job_on(
        &tool.places,
        ssh,
        &picked.host,
        picked.os,
        &source,
        &request,
    )? {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
