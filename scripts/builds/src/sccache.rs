//! The shared sccache server: which process listens on its port, whether
//! launchd's job is that process, what it says about itself, the compiles it
//! runs, and stopping it. Only a server that is already listening is asked:
//! asking with none running starts one.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::fail;
use crate::fail::{Context, Result};

/// The TCP port the server listens on: sccache's default.
pub const PORT: u16 = 4226;

/// How often the waits below look again.
const POLL: Duration = Duration::from_millis(250);

/// The arguments of the lsof that prints the pid listening on sccache's
/// port.
fn listener_args() -> [String; 4] {
    [
        "-nP".to_string(),
        format!("-iTCP:{PORT}"),
        "-sTCP:LISTEN".to_string(),
        "-t".to_string(),
    ]
}

/// A shell command that prints the pid listening on sccache's port, as
/// `parse_listener` reads it, for a script that runs on a host.
pub fn listener_script() -> String {
    format!("lsof {}", listener_args().join(" "))
}

/// Reads the pid the lsof of `listener_args` printed, or None when it
/// printed none.
pub fn parse_listener(text: &str) -> Option<u32> {
    text.lines().find_map(|line| line.trim().parse().ok())
}

/// The pid of the process listening on sccache's port, or None when no
/// process listens there.
pub fn listener() -> Option<u32> {
    let output = Command::new("lsof")
        .args(listener_args())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_listener(&String::from_utf8_lossy(&output.stdout))
}

/// What `launchctl print` says about a job launchd has loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// `running`, `spawn scheduled`, `not running` and so on.
    pub state: String,
    /// The pid of the job's process while it runs.
    pub pid: Option<u32>,
    /// How the job's process last exited, as launchctl words it.
    pub last_exit: Option<String>,
}

impl Job {
    /// The pid of the job's process, when the job is running.
    pub fn running_pid(&self) -> Option<u32> {
        (self.state == "running").then_some(self.pid).flatten()
    }

    /// `state = spawn scheduled, last exit code = 2`.
    pub fn text(&self) -> String {
        match &self.last_exit {
            Some(exit) => format!("state = {}, last exit code = {exit}", self.state),
            None => format!("state = {}", self.state),
        }
    }
}

/// Reads what `launchctl print <service>` printed, here or on a host
/// (`host_installed::installed_script`). The job's own fields are
/// indented by one tab; deeper lines belong to nested sections, some of which
/// have a `state` of their own.
pub fn parse_job(text: &str) -> Job {
    let field = |key: &str| {
        text.lines()
            .find_map(|line| {
                line.strip_prefix('\t')?
                    .strip_prefix(key)?
                    .strip_prefix(" = ")
            })
            .map(|value| value.trim().to_string())
    };
    Job {
        state: field("state").unwrap_or_default(),
        pid: field("pid").and_then(|pid| pid.parse().ok()),
        last_exit: field("last exit code"),
    }
}

/// What launchd says about <service>, or None when launchd has not loaded
/// it.
pub fn read_job(service: &str) -> Result<Option<Job>> {
    let output = Command::new("launchctl")
        .args(["print", service])
        .stderr(Stdio::null())
        .output()
        .context(|| "running launchctl".to_string())?;
    Ok(output
        .status
        .success()
        .then(|| parse_job(&String::from_utf8_lossy(&output.stdout))))
}

/// Does launchd's job serve sccache's port: is the process listening there
/// the job's running process? Setup (`install::run_sccache_under_launchd`)
/// and the check of what each host has installed (host_installed.rs) both
/// judge the server by this.
pub fn launchd_serves(job: Option<&Job>, listener: Option<u32>) -> bool {
    job.and_then(Job::running_pid)
        .is_some_and(|pid| listener == Some(pid))
}

/// Is a server listening on sccache's port?
pub fn listening() -> bool {
    listener().is_some()
}

/// How many compiles the server with pid <pid> runs now. The server runs
/// each compile as a child process, so this counts its children.
pub fn compiles(pid: u32) -> Result<usize> {
    let output = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .context(|| "running pgrep".to_string())?;
    // pgrep exits 1 when it finds no process.
    match output.status.code() {
        Some(0 | 1) => Ok(String::from_utf8_lossy(&output.stdout).lines().count()),
        _ => fail!("pgrep -P {pid} failed ({})", output.status),
    }
}

/// Waits until the server with pid <pid> runs no compile, so stopping it fails
/// none. Fails when it still runs compiles after <limit>.
pub fn wait_for_compiles(pid: u32, limit: Duration) -> Result<()> {
    let start = Instant::now();
    let mut said = false;
    loop {
        let running = compiles(pid)?;
        if running == 0 {
            return Ok(());
        }
        if start.elapsed() >= limit {
            fail!(
                "the sccache server (pid {pid}) still runs {} after {} seconds, so builds did not stop it; run the same builds command again once no compile runs",
                crate::text::counted(running, "compile", "compiles"),
                limit.as_secs()
            );
        }
        if !said {
            crate::text::note(format!(
                "waiting for the sccache server (pid {pid}) to finish its {}",
                crate::text::counted(running, "compile", "compiles")
            ));
            said = true;
        }
        std::thread::sleep(POLL);
    }
}

/// Stops the server listening on sccache's port with <sccache>
/// `--stop-server`, which lets the requests in progress finish for up to 10
/// seconds, and waits until no process listens there. Fails, with what
/// sccache said, when the port is still held after <limit>.
pub fn stop(sccache: &Path, limit: Duration) -> Result<()> {
    let output = Command::new(sccache)
        .arg("--stop-server")
        .output()
        .context(|| "stopping the sccache server".to_string())?;
    let start = Instant::now();
    while let Some(pid) = listener() {
        if start.elapsed() >= limit {
            fail!(
                "the sccache server (pid {pid}) still listens on port {PORT} {} seconds after sccache --stop-server, which said: {}",
                limit.as_secs(),
                String::from_utf8_lossy(&[output.stdout, output.stderr].concat()).trim()
            );
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

/// The lines `sccache --show-stats` prints; empty when it can't be asked.
pub fn stats() -> Vec<String> {
    let Ok(output) = Command::new("sccache")
        .arg("--show-stats")
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The count after <label> in <stats> ("Compile requests executed  152034").
pub fn count(stats: &[String], label: &str) -> Option<u64> {
    stats.iter().find_map(|line| {
        let rest = line.strip_prefix(label)?;
        if !rest.starts_with(' ') {
            return None;
        }
        rest.trim().parse().ok()
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// What `launchctl print` prints for a running job and for one launchd
    /// keeps starting again, cut to the lines that matter and one nested
    /// section with a `state` of its own.
    pub const RUNNING: &str = "gui/501/local.sccache-server = {\n\tactive count = 1\n\tstate = running\n\n\tprogram = /bin/sh\n\tpid = 2914\n\tlast exit code = (never exited)\n\tendpoints = {\n\t\tstate = active\n\t}\n}\n";
    pub const RESPAWNING: &str = "gui/501/local.sccache-server = {\n\tendpoints = {\n\t\tstate = active\n\t}\n\tstate = spawn scheduled\n\tlast exit code = 2\n}\n";

    #[test]
    fn reads_what_launchctl_says_about_a_job() {
        let running = parse_job(RUNNING);
        assert_eq!(
            running,
            Job {
                state: "running".to_string(),
                pid: Some(2914),
                last_exit: Some("(never exited)".to_string()),
            }
        );
        assert_eq!(running.running_pid(), Some(2914));
        let respawning = parse_job(RESPAWNING);
        assert_eq!(respawning.state, "spawn scheduled");
        assert_eq!(respawning.running_pid(), None);
        assert_eq!(
            respawning.text(),
            "state = spawn scheduled, last exit code = 2"
        );
    }

    /// launchd's job serves sccache's port only when its running process is
    /// the one listening there.
    #[test]
    fn launchd_serves_only_when_its_process_listens() {
        let running = parse_job(RUNNING);
        assert!(launchd_serves(Some(&running), Some(2914)));
        assert!(
            !launchd_serves(Some(&running), Some(77)),
            "a client's server"
        );
        assert!(!launchd_serves(Some(&running), None), "not listening yet");
        assert!(!launchd_serves(Some(&parse_job(RESPAWNING)), Some(77)));
        assert!(!launchd_serves(None, Some(77)), "launchd has no job");
    }

    /// A process running `sh -c <script>` in a process group of its own, so
    /// the test can stop it and its children together.
    struct Group(std::process::Child);

    impl Group {
        fn start(script: &str) -> Group {
            use std::os::unix::process::CommandExt;
            Group(
                Command::new("sh")
                    .args(["-c", script])
                    .process_group(0)
                    .spawn()
                    .unwrap(),
            )
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }
    }

    impl Drop for Group {
        fn drop(&mut self) {
            // SAFETY: kill(2) on the group this test started.
            unsafe { libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }

    /// The wait returns once the process has no child left, and fails, naming
    /// the process and its compiles, when a child outlasts the limit.
    #[test]
    fn waits_for_a_servers_compiles_to_finish() {
        // sh runs `sleep 0.5` as a child, then becomes `sleep 30` itself.
        let finishing = Group::start("sleep 0.5; exec sleep 30");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(compiles(finishing.pid()).unwrap(), 1);
        wait_for_compiles(finishing.pid(), Duration::from_secs(10)).unwrap();
        assert_eq!(compiles(finishing.pid()).unwrap(), 0);

        let busy = Group::start("sleep 30; exit 0");
        std::thread::sleep(Duration::from_millis(100));
        let Err(crate::fail::Fail::Message(message)) =
            wait_for_compiles(busy.pid(), Duration::from_millis(300))
        else {
            panic!("expected a failure");
        };
        assert!(
            message.contains(&format!("(pid {}) still runs 1 compile", busy.pid())),
            "{message}"
        );
    }

    #[test]
    fn counts() {
        let stats = vec![
            "Compile requests                  209331".to_string(),
            "Compile requests executed         153775".to_string(),
        ];
        assert_eq!(count(&stats, "Compile requests executed"), Some(153775));
        assert_eq!(count(&stats, "Compile requests"), Some(209331));
        assert_eq!(count(&stats, "Cache hits"), None);
    }
}
