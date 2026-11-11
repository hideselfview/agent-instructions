//! The app lock's queue, end to end: real `builds lock` processes on a scratch
//! build volume, with a shell loop and `sleep` as the build. Each test has a
//! root of its own, so they run side by side.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");
const PATIENCE: Duration = Duration::from_secs(30);
const LABELS: [&str; 5] = ["H", "A", "B", "C", "X"];

/// A scratch build volume with a repo `demo` that has an app checkout; the
/// builds write their log and gates in its folder too.
struct Volume {
    dir: PathBuf,
    started: Vec<Child>,
}

/// The build: logs `start <label>`, starts a background sleep whose pid it
/// writes down, waits for the gate file `go-<label>` to appear, then stops the
/// sleep and logs `end <label>`. Stopped by SIGTERM it logs `stop <label>`.
const BUILD: &str = r#"
label=$0; dir=$1
trap 'echo "stop $label" >> "$dir/log"; exit 1' TERM
echo "start $label" >> "$dir/log"
sleep 300 &
background=$!
# Until it has become the sleep: a signal before that is lost.
while [ "$(ps -o comm= -p $background)" != sleep ]; do :; done
echo $background > "$dir/background-$label"
while [ ! -e "$dir/go-$label" ]; do sleep 0.05; done
trap - TERM
kill $background
wait $background
echo "end $label" >> "$dir/log"
"#;

impl Volume {
    fn new(name: &str) -> Volume {
        // Short: the dashboard's socket path may not pass 104 bytes, which the
        // user's temp folder alone nearly does.
        let dir = PathBuf::from(format!("/private/tmp/bq-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("volume/demo")).unwrap();
        std::fs::write(dir.join("volume/demo/app-checkout"), "side-1\n").unwrap();
        Volume { dir, started: Vec::new() }
    }

    fn builds(&self) -> Command {
        let mut command = Command::new(BUILDS);
        command
            .env("BUILDS_ROOT", self.dir.join("volume"))
            .env("HOME", self.dir.join("home"))
            .env("QUEUE_TEST_DIR", &self.dir);
        command
    }

    /// Starts `builds lock demo` running the build as <label>; its pid.
    fn lock(&mut self, label: &str) -> i32 {
        let child = self
            .builds()
            .args(["lock", "demo", "sh", "-c", BUILD, label])
            .arg(&self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        self.started.push(child);
        pid
    }

    /// Starts <args> as a `builds` command that is queued with the others;
    /// its pid.
    fn spawn(&mut self, args: &[&str]) -> i32 {
        let child = self.builds().args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let pid = child.id() as i32;
        self.started.push(child);
        pid
    }

    /// A repo `demo` with an origin and a `.app-build` of the steps one and
    /// two (scripts that log like `BUILD` and wait for `go-one`, `go-two`),
    /// and its app checkout recorded.
    fn app_repo(&self) {
        let top = self.dir.join("checkout");
        let git = |args: &[&str]| {
            let status = Command::new("git").arg("-C").arg(&top).args(args).output().unwrap();
            assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
        };
        std::fs::create_dir_all(&top).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        for name in ["one", "two"] {
            let script = format!(
                "#!/bin/sh\ntrap 'echo \"stop {name}\" >> \"$QUEUE_TEST_DIR/log\"; exit 1' TERM\necho \"start {name}\" >> \"$QUEUE_TEST_DIR/log\"\nwhile [ ! -e \"$QUEUE_TEST_DIR/go-{name}\" ]; do sleep 0.05; done\necho \"end {name}\" >> \"$QUEUE_TEST_DIR/log\"\n"
            );
            let path = top.join(format!("{name}.sh"));
            std::fs::write(&path, script).unwrap();
            let mut mode = std::fs::metadata(&path).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
            std::fs::set_permissions(&path, mode).unwrap();
        }
        std::fs::write(
            top.join(".app-build"),
            "mac build ./one.sh\nmac app out.app\nmac bundle demo.test\nstep one ./one.sh\nstep two ./two.sh\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "Start"]);
        let origin = self.dir.join("origin.git");
        let made = Command::new("git").args(["init", "-q", "--bare"]).arg(&origin).output().unwrap();
        assert!(made.status.success());
        git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&["push", "-q", "origin", "main"]);
        std::fs::create_dir_all(self.dir.join("volume/demo/side-1")).unwrap();
        std::fs::write(self.dir.join("volume/demo/side-1/.checkout"), top.to_str().unwrap()).unwrap();
    }

    fn order(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("volume/demo/app.queue/order"))
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Waits until the order holds <count> tickets.
    fn until_queued(&self, count: usize) {
        until(&format!("{count} tickets in the order"), || self.order().len() == count);
    }

    /// Lets the build <label> finish.
    fn open_gate(&self, label: &str) {
        std::fs::write(self.dir.join(format!("go-{label}")), "").unwrap();
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("log"))
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn until_logged(&self, line: &str) {
        until(&format!("{line:?} in the log"), || self.log().iter().any(|l| l == line));
    }

    /// The log once it has <count> lines.
    fn log_of(&self, count: usize) -> Vec<String> {
        let deadline = Instant::now() + PATIENCE;
        while self.log().len() < count {
            assert!(
                Instant::now() < deadline,
                "never saw {count} lines in the log: {:?}, order {:?}",
                self.log(),
                self.order()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        self.log()
    }

    fn background(&self, label: &str) -> i32 {
        std::fs::read_to_string(self.dir.join(format!("background-{label}"))).unwrap().trim().parse().unwrap()
    }

    /// Runs a `builds` command to its end.
    fn run(&self, args: &[&str]) -> std::process::Output {
        self.builds().args(args).output().unwrap()
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        for child in &mut self.started {
            // SIGTERM: a running `builds lock` passes it to its build's group.
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            let _ = child.wait();
        }
        for label in LABELS {
            let text = std::fs::read_to_string(self.dir.join(format!("background-{label}"))).unwrap_or_default();
            if let Ok(pid) = text.trim().parse::<i32>() {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "never saw {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Fails when two builds ran at once: every `start` needs the one before it
/// to have ended or been stopped.
fn assert_one_at_a_time(log: &[String]) {
    let mut running = 0;
    for line in log {
        running += if line.starts_with("start ") { 1 } else { -1 };
        assert!(running <= 1, "two builds ran at once in {log:?}");
    }
}

fn lines(log: &[&str]) -> Vec<String> {
    log.iter().map(|line| line.to_string()).collect()
}

fn open_all_gates(volume: &Volume) {
    for label in LABELS {
        volume.open_gate(label);
    }
}

/// Starts H, which holds the lock, then each of <waiters> in turn; the pids,
/// H's first.
fn queue_up(volume: &mut Volume, waiters: &[&str]) -> Vec<i32> {
    let mut pids = vec![volume.lock("H")];
    volume.until_logged("start H");
    for (at, label) in waiters.iter().enumerate() {
        pids.push(volume.lock(label));
        volume.until_queued(at + 2);
    }
    pids
}

#[test]
fn waiters_go_in_the_order_they_came() {
    let mut volume = Volume::new("order");
    queue_up(&mut volume, &["A", "B", "C"]);
    open_all_gates(&volume);
    let log = volume.log_of(8);
    assert_eq!(log, lines(&["start H", "end H", "start A", "end A", "start B", "end B", "start C", "end C"]));
}

#[test]
fn top_moves_a_waiter_ahead_of_the_others() {
    let mut volume = Volume::new("top");
    let pids = queue_up(&mut volume, &["A", "B", "C"]);
    let moved = volume.run(&["queue", "demo", "top", &pids[3].to_string()]);
    assert!(moved.status.success(), "{}", String::from_utf8_lossy(&moved.stderr));
    let shown = String::from_utf8(volume.run(&["queue", "demo"]).stdout).unwrap();
    let in_line = |pid: i32| shown.find(&pid.to_string()).unwrap_or_else(|| panic!("{pid} not in:\n{shown}"));
    assert!(in_line(pids[3]) < in_line(pids[1]) && in_line(pids[1]) < in_line(pids[2]), "next first:\n{shown}");
    open_all_gates(&volume);
    let log = volume.log_of(8);
    assert_eq!(log, lines(&["start H", "end H", "start C", "end C", "start A", "end A", "start B", "end B"]));
}

#[test]
fn a_waiter_killed_in_line_lets_nobody_past_the_holder() {
    let mut volume = Volume::new("killed");
    let pids = queue_up(&mut volume, &["A", "B"]);
    unsafe { libc::kill(pids[1], libc::SIGKILL) };
    // B waited on A's ticket. A's death frees it, B takes it away and goes on
    // to wait on the holder's.
    until("A's ticket gone from the order", || volume.order().len() == 2);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(volume.log(), lines(&["start H"]), "B started while H still held the lock");
    volume.open_gate("B");
    volume.open_gate("H");
    let log = volume.log_of(4);
    assert_eq!(log, lines(&["start H", "end H", "start B", "end B"]));
}

#[test]
fn takeover_stops_the_holder_runs_the_taker_then_runs_the_holder_again() {
    let mut volume = Volume::new("takeover");
    let pids = queue_up(&mut volume, &["A", "B"]);
    let background = volume.background("H");
    assert!(alive(background));
    volume.open_gate("A");
    volume.open_gate("B");
    let taken = volume.run(&["takeover", "demo", &pids[2].to_string()]);
    assert!(taken.status.success(), "{}", String::from_utf8_lossy(&taken.stderr));
    // H's whole group was stopped, its background sleep too, before B starts.
    volume.until_logged("start B");
    assert!(!alive(background), "the stopped build's background process lives");
    volume.until_logged("stop H");
    volume.open_gate("H");
    let log = volume.log_of(8);
    assert_eq!(log, lines(&["start H", "stop H", "start B", "end B", "start H", "end H", "start A", "end A"]));
    assert_one_at_a_time(&log);
}

#[test]
fn takeover_without_a_pid_takes_the_first_waiter() {
    let mut volume = Volume::new("takeover-first");
    queue_up(&mut volume, &["A", "B"]);
    volume.open_gate("A");
    volume.open_gate("B");
    let taken = volume.run(&["takeover", "demo"]);
    assert!(taken.status.success(), "{}", String::from_utf8_lossy(&taken.stderr));
    volume.until_logged("end A");
    volume.until_logged("stop H");
    volume.open_gate("H");
    let log = volume.log_of(8);
    assert_eq!(log, lines(&["start H", "stop H", "start A", "end A", "start H", "end H", "start B", "end B"]));
    assert_one_at_a_time(&log);
}

/// One request to the dashboard at <port>, answered with its status line.
fn request(port: u16, method: &str, path: &str, origin: Option<&str>, body: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let origin = origin.map(|origin| format!("Origin: {origin}\r\n")).unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply.lines().next().unwrap_or("").to_string()
}

#[test]
fn the_dashboard_moves_a_waiter_only_for_its_own_page() {
    let mut volume = Volume::new("dashboard");
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    let dashboard = volume.builds().args(["dashboard", "--port", &port.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    volume.started.push(dashboard);
    until("the dashboard listening", || TcpStream::connect(("127.0.0.1", port)).is_ok());
    let pids = queue_up(&mut volume, &["A", "B"]);
    let body = format!(r#"{{"action": "top", "repo": "demo", "pid": {}}}"#, pids[2]);
    let own = format!("http://127.0.0.1:{port}");

    assert!(request(port, "POST", "/api/queue", None, &body).contains("403"), "no Origin");
    assert!(request(port, "POST", "/api/queue", Some("http://evil.example"), &body).contains("403"), "another page");
    assert!(request(port, "POST", "/api/other", Some(&own), &body).contains("405"), "another path");
    assert!(request(port, "PUT", "/api/queue", Some(&own), &body).contains("405"), "another method");
    let before = volume.order();
    assert_eq!(before.len(), 3);

    assert!(request(port, "POST", "/api/queue", Some(&own), &body).contains("200"));
    let order = volume.order();
    assert_ne!(order, before, "B moved");
    assert_eq!(order[1].split('-').next().unwrap(), pids[2].to_string(), "B is behind the holder: {order:?}");
    let bad = format!(r#"{{"action": "top", "repo": "../demo", "pid": {}}}"#, pids[1]);
    assert!(request(port, "POST", "/api/queue", Some(&own), &bad).contains("500"), "a path as the repo");
}

/// Waiters A, B and X behind H, then `top` of X, over and over: X waited on
/// B's ticket, and if it missed the signal that re-points it, B would wait on
/// X's and A on B's, a cycle nothing breaks.
#[test]
fn moving_the_last_waiter_to_the_top_leaves_no_cycle() {
    for round in 0..10 {
        let mut volume = Volume::new(&format!("cycle-{round}"));
        let pids = queue_up(&mut volume, &["A", "B", "X"]);
        let moved = volume.run(&["queue", "demo", "top", &pids[3].to_string()]);
        assert!(moved.status.success(), "{}", String::from_utf8_lossy(&moved.stderr));
        volume.open_gate("A");
        volume.open_gate("B");
        volume.open_gate("X");
        volume.open_gate("H");
        // log_of bounds the wait.
        let log = volume.log_of(8);
        assert_eq!(log, lines(&["start H", "end H", "start X", "end X", "start A", "end A", "start B", "end B"]), "round {round}");
    }
}

#[test]
fn a_takeover_runs_an_app_build_again_from_its_first_step() {
    let mut volume = Volume::new("app-build");
    volume.app_repo();
    volume.spawn(&["app-build", "demo", "main"]);
    volume.until_logged("start one");
    volume.lock("W");
    volume.until_queued(2);
    volume.open_gate("W");
    let taken = volume.run(&["takeover", "demo"]);
    assert!(taken.status.success(), "{}", String::from_utf8_lossy(&taken.stderr));
    volume.until_logged("end W");
    volume.until_logged("stop one");
    std::fs::write(volume.dir.join("go-one"), "").unwrap();
    std::fs::write(volume.dir.join("go-two"), "").unwrap();
    let log = volume.log_of(8);
    assert_eq!(log, lines(&["start one", "stop one", "start W", "end W", "start one", "end one", "start two", "end two"]));
    assert_one_at_a_time(&log);
}

#[test]
fn app_build_runs_the_steps_named_and_lists_them() {
    let volume = Volume::new("app-build-steps");
    volume.app_repo();
    let listed = volume.run(&["app-build", "demo", "--list"]);
    assert_eq!(String::from_utf8_lossy(&listed.stdout), "one ./one.sh\ntwo ./two.sh\n");
    std::fs::write(volume.dir.join("go-two"), "").unwrap();
    let built = volume.run(&["app-build", "demo", "main", "two"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    assert_eq!(volume.log(), lines(&["start two", "end two"]));
    let unknown = volume.run(&["app-build", "demo", "main", "three"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("no step three"));
}
