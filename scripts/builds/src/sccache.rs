//! The shared sccache server: what it says about itself, the compiles it
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

/// The pid of the process listening on sccache's port, or None when no
/// process listens there.
pub fn listener() -> Option<u32> {
    let output = Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{PORT}"), "-sTCP:LISTEN", "-t"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().parse().ok())
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
mod tests {
    use super::*;

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
