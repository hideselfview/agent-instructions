//! The shared sccache server: which process listens on its port, whether
//! launchd's job is that process, what it says about itself, the compiles it
//! runs, and stopping it. This module asks the server about itself only when
//! it already listens, because asking starts a server when none runs.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::fail;
use crate::fail::{Context, Result};

/// The TCP port the server listens on: sccache's default.
pub const PORT: u16 = 4226;

/// How often the waits below look again.
const POLL: Duration = Duration::from_millis(250);

/// The arguments of the lsof that prints, for each process listening on
/// sccache's port, its pid, its parent's pid and its command's name.
fn listener_args() -> [String; 4] {
    [
        "-nP".to_string(),
        format!("-iTCP:{PORT}"),
        "-sTCP:LISTEN".to_string(),
        "-FpRc".to_string(),
    ]
}

/// The pid of launchd, the parent of each process it starts.
const LAUNCHD: u32 = 1;

/// A process listening on sccache's port, its parent and its command.
struct Listener {
    pid: u32,
    parent: u32,
    command: String,
}

/// Reads the processes that the lsof of `listener_args` printed. lsof prints
/// a `p` line with each pid, then an `R` line with its parent's pid, a `c`
/// line with its command's name, and an `f` line for each listening socket
/// the process holds. Returns None when a line is not one of these, or a pid
/// lacks its parent or its command.
fn parse_listeners(stdout: &str) -> Option<Vec<Listener>> {
    struct Read {
        pid: u32,
        parent: Option<u32>,
        command: Option<String>,
    }
    let mut read: Vec<Read> = Vec::new();
    for line in stdout.lines().filter(|line| !line.is_empty()) {
        if let Some(value) = line.strip_prefix('p') {
            read.push(Read {
                pid: value.parse().ok()?,
                parent: None,
                command: None,
            });
        } else if let Some(value) = line.strip_prefix('R') {
            read.last_mut()?.parent = Some(value.parse().ok()?);
        } else if let Some(value) = line.strip_prefix('c') {
            read.last_mut()?.command = Some(value.to_string());
        } else if !line.starts_with('f') {
            return None;
        }
    }
    read.into_iter()
        .map(|process| {
            Some(Listener {
                pid: process.pid,
                parent: process.parent?,
                command: process.command?,
            })
        })
        .collect()
}

/// The pid of the sccache server among <listeners>. The server runs each
/// compile as a child process, and each child holds the server's listening
/// socket, so lsof lists the server and its compiles. The server is the one
/// listener whose parent does not listen. When lsof lists several such
/// servers, as when a client started a server of its own beside launchd's
/// for a moment, fails naming each, which launchd started and which it did
/// not.
fn server(listeners: &[Listener]) -> Result<u32> {
    let servers: Vec<&Listener> = listeners
        .iter()
        .filter(|listener| !listeners.iter().any(|other| other.pid == listener.parent))
        .collect();
    match servers[..] {
        [server] => Ok(server.pid),
        _ => fail!(
            "{} sccache servers listen on port {PORT}: {}",
            servers.len(),
            servers
                .iter()
                .map(|server| if server.parent == LAUNCHD {
                    format!(
                        "pid {} ({}), which launchd started",
                        server.pid, server.command
                    )
                } else {
                    format!(
                        "pid {} ({}), which pid {} started and launchd did not",
                        server.pid, server.command, server.parent
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

/// Reads what the lsof of `listener_args` did, from its exit status and
/// what it printed: the pid of the sccache server among the processes it
/// printed, or None when it found no listener, which lsof reports by exiting
/// 1 and printing nothing. Any other result is a failure that says what lsof
/// said.
fn read_listener(status: Option<i32>, stdout: &str, stderr: &str) -> Result<Option<u32>> {
    match (status, parse_listeners(stdout)) {
        (Some(0), Some(listeners)) if !listeners.is_empty() => server(&listeners).map(Some),
        (Some(1), Some(listeners)) if listeners.is_empty() && stderr.trim().is_empty() => Ok(None),
        _ => fail!(
            "lsof {} failed ({}): {}",
            listener_args().join(" "),
            status.map_or("no exit status".to_string(), |code| format!("exit {code}")),
            format!("{stdout} {stderr}")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

/// A shell script, for a host, that runs the lsof of `listener_args` and
/// prints its exit status on the line `listener_status=`, each line of its
/// stdout after `listener=`, and its stderr on the line `listener_said=`.
/// `parse_listener_lines` reads these lines. When lsof found a listener, the
/// script then prints the server's JSON stats on the line `sccache_stats=`,
/// which `parse_stats_line` reads: the script asks the server only when it
/// listens, since asking starts a server when none runs.
pub fn listener_script() -> String {
    format!(
        r#"listener_err=$(mktemp)
listener=$(lsof {args} 2>"$listener_err")
listener_status=$?
echo "listener_status=$listener_status"
printf '%s\n' "$listener" | sed 's/^/listener=/'
echo "listener_said=$(tr '\n' ' ' <"$listener_err")"
rm -f "$listener_err"
if [ "$listener_status" = 0 ]; then printf 'sccache_stats=%s\n' "$({STATS} 2>&1 | tr '\n' ' ')"; fi"#,
        args = listener_args().join(" ")
    )
}

/// The command that prints the listening server's stats as one line of JSON.
const STATS: &str = "sccache --show-stats --stats-format json";

/// The max cache size, in bytes, of the server whose JSON stats are on the
/// `sccache_stats=` line among <text>'s, or None when no such line is there
/// (no server listened). Fails when the line holds no max cache size.
pub fn parse_stats_line(text: &str) -> Result<Option<u64>> {
    text.lines()
        .find_map(|line| line.strip_prefix("sccache_stats="))
        .map(parse_max_cache_size)
        .transpose()
}

/// The max cache size, in bytes, in the JSON stats <json> that the server
/// printed. Fails, with what it printed, when the JSON holds none.
pub fn parse_max_cache_size(json: &str) -> Result<u64> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|stats| stats.get("max_cache_size")?.as_u64())
        .map_or_else(
            || fail!("{STATS} printed no max cache size: {}", json.trim()),
            Ok,
        )
}

/// The max cache size, in bytes, that the server listening on sccache's port
/// runs with. Ask only a server that listens: asking starts one when none
/// runs. Fails when sccache fails or prints no max cache size.
pub fn running_max_cache_size() -> Result<u64> {
    let mut words = STATS.split_whitespace();
    let output = Command::new(words.next().expect("the command has a program"))
        .args(words)
        .output()
        .context(|| "running sccache --show-stats".to_string())?;
    if !output.status.success() {
        fail!(
            "{STATS} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&[output.stdout, output.stderr].concat()).trim()
        );
    }
    parse_max_cache_size(&String::from_utf8_lossy(&output.stdout))
}

/// Reads the lines `listener_script` printed among <text>'s.
pub fn parse_listener_lines(text: &str) -> Result<Option<u32>> {
    let value = |key: &str| crate::host::answer_value(text, key);
    let stdout: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("listener="))
        .collect();
    let (Some(status), false, Some(stderr)) = (
        value("listener_status"),
        stdout.is_empty(),
        value("listener_said"),
    ) else {
        fail!("the answer has no lsof lines for sccache's port: {text:?}");
    };
    read_listener(status.trim().parse().ok(), &stdout.join("\n"), stderr)
}

/// The pid of the sccache server, the process that listens on sccache's
/// port and whose parent does not, or None when no process listens there.
/// Fails when lsof fails.
pub fn listener() -> Result<Option<u32>> {
    let output = Command::new("lsof")
        .args(listener_args())
        .output()
        .context(|| "running lsof".to_string())?;
    read_listener(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// Is a server listening on sccache's port? Fails when lsof fails.
pub fn listening() -> Result<bool> {
    Ok(listener()?.is_some())
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
/// (`host_installed::installed_script`). launchctl indents the job's own
/// fields by one tab; deeper lines belong to nested sections, some of which
/// have a `state` of their own.
/// Fails when the job has no `state` line or a `pid` that is not a number,
/// since launchctl then printed something builds does not know how to read.
pub fn parse_job(text: &str) -> Result<Job> {
    let field = |key: &str| {
        text.lines()
            .find_map(|line| {
                line.strip_prefix('\t')?
                    .strip_prefix(key)?
                    .strip_prefix(" = ")
            })
            .map(|value| value.trim().to_string())
    };
    let Some(state) = field("state") else {
        fail!("launchctl print printed no state for the job: {text:?}");
    };
    let pid = match field("pid") {
        None => None,
        Some(pid) => match pid.parse() {
            Ok(pid) => Some(pid),
            Err(_) => fail!("launchctl print printed a pid that is not a number: {pid:?}"),
        },
    };
    Ok(Job {
        state,
        pid,
        last_exit: field("last exit code"),
    })
}

/// What launchd says about <service>, or None when launchd has not loaded
/// it.
pub fn read_job(service: &str) -> Result<Option<Job>> {
    let output = Command::new("launchctl")
        .args(["print", service])
        .stderr(Stdio::null())
        .output()
        .context(|| "running launchctl".to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    parse_job(&String::from_utf8_lossy(&output.stdout)).map(Some)
}

/// Does launchd's job serve sccache's port: is the process listening there
/// the job's running process? Setup (`install::run_sccache_under_launchd`)
/// and the check of what each host has installed (host_installed.rs) both
/// judge the server by this.
pub fn launchd_serves(job: Option<&Job>, listener: Option<u32>) -> bool {
    job.and_then(Job::running_pid)
        .is_some_and(|pid| listener == Some(pid))
}

/// How many compiles the server with pid <pid> runs now. The server runs
/// each compile as a child process, so `compiles` counts the server's child
/// processes.
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

/// Waits until the server with pid <pid> runs no compile, so that no compile
/// fails when the server stops. Fails when the server still runs compiles
/// after <limit>.
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

/// How long a stopped server may hold sccache's port: sccache waits up to 10
/// seconds for the requests in progress.
const STOP_LIMIT: Duration = Duration::from_secs(20);

/// Stops the server listening on sccache's port with <sccache>
/// `--stop-server`, which lets the requests in progress finish for up to 10
/// seconds, and waits until no process listens there. Fails, with what
/// sccache said, when a process still listens on the port after STOP_LIMIT.
pub fn stop(sccache: &Path) -> Result<()> {
    let output = Command::new(sccache)
        .arg("--stop-server")
        .output()
        .context(|| "stopping the sccache server".to_string())?;
    let start = Instant::now();
    while let Some(pid) = listener()? {
        if start.elapsed() >= STOP_LIMIT {
            fail!(
                "the sccache server (pid {pid}) still listens on port {PORT} {} seconds after sccache --stop-server, which said: {}",
                STOP_LIMIT.as_secs(),
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
        let running = parse_job(RUNNING).unwrap();
        assert_eq!(
            running,
            Job {
                state: "running".to_string(),
                pid: Some(2914),
                last_exit: Some("(never exited)".to_string()),
            }
        );
        assert_eq!(running.running_pid(), Some(2914));
        let respawning = parse_job(RESPAWNING).unwrap();
        assert_eq!(respawning.state, "spawn scheduled");
        assert_eq!(respawning.running_pid(), None);
        assert_eq!(
            respawning.text(),
            "state = spawn scheduled, last exit code = 2"
        );
    }

    /// Output with no state for the job, or a pid that is not a number, is a
    /// failure, not a job with an empty state or no pid.
    #[test]
    fn launchctl_output_builds_cannot_read_is_an_error() {
        assert!(parse_job("gui/501/x = {\n\tendpoints = {\n\t\tstate = active\n\t}\n}\n").is_err());
        assert!(parse_job("\tstate = running\n\tpid = twelve\n").is_err());
        assert_eq!(parse_job("\tstate = not running\n").unwrap().pid, None);
    }

    /// launchd's job serves sccache's port only when its running process is
    /// the one listening there.
    #[test]
    fn launchd_serves_only_when_its_process_listens() {
        let running = parse_job(RUNNING).unwrap();
        assert!(launchd_serves(Some(&running), Some(2914)));
        assert!(
            !launchd_serves(Some(&running), Some(77)),
            "a client's server"
        );
        assert!(!launchd_serves(Some(&running), None), "not listening yet");
        assert!(!launchd_serves(
            Some(&parse_job(RESPAWNING).unwrap()),
            Some(77)
        ));
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

        /// Returns once sh has started its one child, and panics when it has
        /// not after 10 seconds.
        fn wait_for_child(&self) {
            let start = std::time::Instant::now();
            while compiles(self.pid()).unwrap() != 1 {
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "sh (pid {}) started no child in 10 seconds",
                    self.pid()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
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
        finishing.wait_for_child();
        wait_for_compiles(finishing.pid(), Duration::from_secs(10)).unwrap();
        assert_eq!(compiles(finishing.pid()).unwrap(), 0);

        let busy = Group::start("sleep 30; exit 0");
        busy.wait_for_child();
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

    /// lsof's exit 1 with nothing printed means nothing listens; any other
    /// failure of lsof is an error, never "nothing listens".
    #[test]
    fn reads_what_lsof_said_about_the_port() {
        assert_eq!(
            read_listener(Some(0), "p2914\nR1\ncsccache\nf12\n", "").unwrap(),
            Some(2914)
        );
        assert_eq!(read_listener(Some(1), "", "").unwrap(), None);
        let failed = |status, stdout, stderr| match read_listener(status, stdout, stderr) {
            Err(crate::fail::Fail::Message(message)) => message,
            other => panic!("expected a failure, got {other:?}"),
        };
        assert!(failed(Some(1), "", "lsof: no pwd entry").contains("exit 1"));
        assert!(failed(Some(1), "", "lsof: no pwd entry").contains("lsof: no pwd entry"));
        assert!(failed(Some(127), "", "sh: lsof: not found").contains("exit 127"));
        assert!(failed(None, "", "").contains("no exit status"));
        assert!(failed(Some(0), "garbage", "").contains("garbage"));
        assert!(failed(Some(0), "p2914\n", "").contains("p2914"));
        let printed = "x=1\nlistener_status=0\nlistener=p2914\nlistener=R1\nlistener=csccache\nlistener=f12\nlistener_said=\n";
        assert_eq!(parse_listener_lines(printed).unwrap(), Some(2914));
        assert_eq!(
            parse_listener_lines("listener_status=1\nlistener=\nlistener_said=\n").unwrap(),
            None
        );
        assert!(
            parse_listener_lines("listener_status=1\nlistener=\nlistener_said=lsof: denied\n")
                .is_err()
        );
        assert!(parse_listener_lines("listener=2914\n").is_err());
    }

    /// What lsof prints while the server (pid 44790) runs two compiles, which
    /// hold its listening socket too.
    const COMPILING: &str =
        "p71743\nR44790\ncclang\nf5\np44790\nR1\ncsccache\nf5\np71754\nR44790\ncclang\nf5\n";

    #[test]
    fn reads_the_server_among_its_compiles() {
        assert_eq!(read_listener(Some(0), COMPILING, "").unwrap(), Some(44790));
        assert_eq!(
            read_listener(
                Some(0),
                "p600\nR500\ncclang\nf5\np500\nR400\ncsccache\nf5\n",
                ""
            )
            .unwrap(),
            Some(500),
            "a client's server, whose parent is the client"
        );
    }

    /// A second server beside launchd's is named apart from a failure of
    /// lsof: the message names both servers, says which one launchd started,
    /// and names the parent of the one it did not.
    #[test]
    fn names_a_second_server_beside_launchds() {
        let Err(crate::fail::Fail::Message(message)) = read_listener(
            Some(0),
            "p2914\nR1\ncsccache\nf12\np92600\nR9100\ncsccache\nf5\n",
            "",
        ) else {
            panic!("expected a failure");
        };
        assert!(!message.contains("lsof"), "{message}");
        assert!(
            message.contains("2 sccache servers listen on port 4226"),
            "{message}"
        );
        assert!(
            message.contains("pid 2914 (sccache), which launchd started"),
            "{message}"
        );
        assert!(
            message.contains("pid 92600 (sccache), which pid 9100 started and launchd did not"),
            "{message}"
        );
    }

    #[test]
    fn reads_a_hosts_server_among_its_compiles() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = crate::testing::Scratch::new("sccache-listeners");
        let lsof = scratch.dir.join("lsof");
        std::fs::write(&lsof, format!("#!/bin/sh\ncat <<'EOF'\n{COMPILING}EOF\n")).unwrap();
        std::fs::set_permissions(&lsof, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new("sh")
            .args(["-c", &listener_script()])
            .env("PATH", format!("{}:/usr/bin:/bin", scratch.dir.display()))
            .output()
            .unwrap();
        assert_eq!(
            parse_listener_lines(&String::from_utf8_lossy(&output.stdout)).unwrap(),
            Some(44790)
        );
    }

    /// The running server's max cache size comes from its JSON stats; JSON
    /// without it is an error, never a cap of nothing.
    #[test]
    fn reads_the_running_servers_max_cache_size() {
        assert_eq!(
            parse_max_cache_size(
                "{\"stats\":{\"compile_requests\":5},\"cache_size\":10,\"max_cache_size\":32212254720}"
            )
            .unwrap(),
            32212254720
        );
        assert!(parse_max_cache_size("{\"stats\":{}}").is_err());
        assert!(parse_max_cache_size("sccache: error").is_err());
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
