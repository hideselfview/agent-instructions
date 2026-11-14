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
use crate::host::{self, Host, Ssh, choose, clone_path, sh_quote};
use crate::places::Places;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds remote run <repo> <ref> [--host <name>] -- <command> [<args>...]\n       builds remote run <repo> --staged [--host <name>] -- <command> [<args>...]";

/// The refs a job's commit is sent as, in the host's clone.
const JOBS_REF: &str = "refs/builds/jobs";

/// The exit status builds gives a job a signal stopped.
const STOPPED_EXIT: i32 = 130;

/// Set by the signal handler: the person or a parent stopped this command.
static STOPPED: AtomicBool = AtomicBool::new(false);

extern "C" fn note_stop(_: libc::c_int) {
    STOPPED.store(true, Ordering::SeqCst);
}

/// What a job runs.
#[derive(Debug, PartialEq, Eq)]
pub enum What {
    Ref(String),
    Staged,
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

/// The checkout the job's commit comes from: the one the command runs in when
/// it is a checkout of <repo>, else `~/dev/<repo>`.
fn source_of(places: &Places, repo: &str, what: &What) -> Result<PathBuf> {
    let here = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    if let Some(checkout) = Checkout::at(&here)?
        && checkout.repo() == repo
    {
        return Ok(checkout.top);
    }
    if *what == What::Staged {
        fail!(
            "--staged sends the staged files of the checkout the command runs in, and this is not a checkout of {repo}"
        );
    }
    let main = places.home.join(clone_path(repo));
    if !main.join(".git").exists() {
        fail!("{} is no clone of {repo}", main.display());
    }
    Ok(main)
}

/// The commit the job runs: <rev>'s, or a new commit holding the staged
/// files. The temporary commit moves no ref; nothing in the checkout changes.
fn commit_of(source: &Path, what: &What) -> Result<String> {
    let text = match what {
        What::Ref(rev) => git_text(
            git(source).args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")]),
            &format!("resolve {rev}"),
        )?,
        What::Staged => {
            let tree = git_text(
                git(source).arg("write-tree"),
                "write the staged files' tree",
            )?;
            git_text(
                git(source).args([
                    "-c",
                    "user.name=builds",
                    "-c",
                    "user.email=builds@localhost",
                    "commit-tree",
                    tree.trim(),
                    "-p",
                    "HEAD",
                    "-m",
                    "builds job: the staged files",
                ]),
                "commit the staged files",
            )?
        }
    };
    Ok(text.trim().to_string())
}

/// Where a job's commands run: this Mac, or a host over ssh.
struct Transport<'a> {
    places: &'a Places,
    ssh: &'a Ssh,
    host: &'a Host,
}

impl Transport<'_> {
    /// A command that runs <script> with `sh` on the host.
    fn command(&self, script: &str) -> Command {
        match &self.host.target {
            Some(target) => self
                .ssh
                .command(target, &format!("sh -c {}", sh_quote(script))),
            None => {
                let mut command = Command::new("sh");
                command.arg("-c").arg(script).env("HOME", &self.places.home);
                command
            }
        }
    }

    /// Runs <script> to its end and returns its stdout; a failure names the
    /// host and what the script printed.
    fn capture(&self, script: &str, what: &str) -> Result<String> {
        let output = self
            .command(script)
            .stdin(Stdio::null())
            .output()
            .context(|| format!("running {what} on host {}", self.host.name))?;
        if !output.status.success() {
            fail!(
                "{what} failed on host {} ({}): {}",
                self.host.name,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Sends <sha> to the host's clone as the ref of job <id>.
    fn send(&self, source: &Path, repo: &str, sha: &str, id: &str) -> Result<()> {
        let Some(target) = &self.host.target else {
            return Ok(());
        };
        let output = git(source)
            .args(["push", "-q", &format!("{target}:{}", clone_path(repo))])
            .arg(format!("{sha}:{JOBS_REF}/{id}"))
            .env("GIT_SSH_COMMAND", self.ssh.git_command())
            .stdin(Stdio::null())
            .output()
            .context(|| "running git push".to_string())?;
        if !output.status.success() {
            fail!(
                "sending the commit to host {} ({target}) failed: {}",
                self.host.name,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }
}

/// Finds the repo's clone and claims the first free worktree for job <id>,
/// with <sha> checked out in it. Prints `worktree=<path>`.
fn claim_script(repo: &str, sha: &str, id: &str) -> String {
    format!(
        r#"set -e
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
echo "worktree=$wt""#,
        clone = clone_path(repo),
        repo = repo,
        sha = sh_quote(sha),
        id = sh_quote(id),
    )
}

/// Runs <command> in <worktree> as a process group of its own, whose leader's
/// pid goes in the claim folder for `stop_script`.
fn run_script(worktree: &str, command: &[String]) -> String {
    let words: Vec<String> = command.iter().map(|word| sh_quote(word)).collect();
    format!(
        r#"PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH
set -m
cd {wt} || exit 92
( exec {words} ) &
pid=$!
echo $pid > {wt}.job/pid
wait $pid 2>/dev/null"#,
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
fn release_script(repo: &str, worktree: Option<&str>, id: &str) -> String {
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
/// returns its exit status. A stop signal stops the command on the host.
fn run_command(transport: &Transport, worktree: &str, command: &[String]) -> Result<i32> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    STOPPED.store(false, Ordering::SeqCst);
    unsafe {
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(signal, note_stop as *const () as libc::sighandler_t);
        }
    }
    let mut child = transport
        .command(&run_script(worktree, command))
        .stdin(Stdio::null())
        // Its own process group: a Ctrl-C reaches builds, which stops the
        // command on the host itself.
        .process_group(0)
        .spawn()
        .context(|| format!("running the command on host {}", transport.host.name))?;
    let mut stopped = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context(|| "waiting for the command".to_string())?
        {
            break status;
        }
        if STOPPED.load(Ordering::SeqCst) && !stopped {
            stopped = true;
            if let Err(error) = transport.capture(&stop_script(worktree), "stopping the command") {
                note(error.text());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if stopped {
        return Ok(STOPPED_EXIT);
    }
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}

/// The worktree path the claim script printed.
fn worktree_in(output: &str) -> Result<String> {
    match output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("worktree="))
    {
        Some(path) if !path.is_empty() => Ok(path.to_string()),
        _ => fail!("the host did not say which worktree it claimed: {output:?}"),
    }
}

/// Sends the commit to <host>, runs the command there and cleans up; the
/// command's exit status.
pub fn run_job(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    source: &Path,
    request: &Request,
) -> Result<i32> {
    let sha = commit_of(source, &request.what)?;
    let id = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_nanos()
    );
    let transport = Transport { places, ssh, host };
    let ran = (|| {
        transport.send(source, &request.repo, &sha, &id)?;
        let claimed = transport.capture(
            &claim_script(&request.repo, &sha, &id),
            "claiming a worktree",
        )?;
        let worktree = worktree_in(&claimed)?;
        let code = run_command(&transport, &worktree, &request.command);
        Ok::<_, Fail>((worktree, code))
    })();
    let worktree = ran.as_ref().ok().map(|(worktree, _)| worktree.as_str());
    let released = transport.capture(
        &release_script(&request.repo, worktree, &id),
        "removing the job's ref and claim",
    );
    let code = match ran {
        Ok((_, code)) => code?,
        Err(error) => {
            if let Err(release) = released {
                note(release.text());
            }
            return Err(error);
        }
    };
    if let Err(error) = released {
        if code == 0 {
            return Err(error);
        }
        note(error.text());
    }
    Ok(code)
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

pub fn cmd_remote(tool: &Tool, ssh: &Ssh, args: &[String]) -> Result<()> {
    let request = parse(args)?;
    let source = source_of(&tool.places, &request.repo, &request.what)?;
    let (host, why) = pick_host(
        &tool.places,
        ssh,
        tool.rules.floor_kib,
        request.host.as_deref(),
    )?;
    note(why);
    match run_job(&tool.places, ssh, &host, &source, &request)? {
        0 => Ok(()),
        code => Err(Fail::Exit(code)),
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
