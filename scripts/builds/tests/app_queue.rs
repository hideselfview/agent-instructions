//! The app lock's queue, end to end: real `builds lock` processes on a scratch
//! build volume, with a shell loop and `sleep` as the build. Each test has a
//! root of its own, so they run side by side.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod common;
mod pruned;
use pruned::pruned_now;
mod test_dir;
use test_dir::test_dir;
mod programs;
use programs::programs_folder;
mod spawned;
use spawned::{Spawned, wait_for_free_lanes};
mod stand_in;
use stand_in::stand_in;

const PATIENCE: Duration = Duration::from_secs(30);
const LABELS: [&str; 5] = ["H", "A", "B", "C", "X"];

/// A scratch build volume with a repo `demo` that has an app checkout; the
/// builds write their log and gates in its folder too.
struct Volume {
    dir: PathBuf,
    pinned_builds: PathBuf,
    started: Vec<Spawned>,
}

/// Writes `xcodebuild`, `ioreg` and `lsappinfo` into the stand-in system
/// <system>, and `sw_vers` into <bin>, which the builds commands find first on
/// PATH: the mac lane reads the host's Xcode, macOS and display from them, so
/// a test never depends on the Mac it runs on (`stand_in::xcode`), or on
/// whether its screen is locked.
fn fake_apple_tools(system: &Path, bin: &Path) {
    stand_in::xcode(system);
    stand_in(system, "ioreg", "echo '\"IOConsoleLocked\" = No'");
    stand_in(
        system,
        "lsappinfo",
        r#"echo '"LSDisplayName"="Terminal" "pid"=1'"#,
    );
    std::fs::create_dir_all(bin).unwrap();
    stand_in(
        bin,
        "sw_vers",
        "if [ \"$1\" = -buildVersion ]; then cat \"$QUEUE_TEST_DIR/macos-build\" 2>/dev/null || echo 26B131; else exit 1; fi",
    );
}

/// Links the folder the volume at <volume> keeps pinned Chrome in to the one
/// the shots crate's tests keep it in, `~/.cache/builds-shots/chrome`. An app
/// build with shots starts Chrome, which downloads it into an empty folder, so
/// each such test would download it again.
fn share_chrome(volume: &Path) {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    let shared = home.join(".cache/builds-shots/chrome");
    std::fs::create_dir_all(&shared).unwrap();
    let cache = volume.join("caches/builds-shots");
    std::fs::create_dir_all(&cache).unwrap();
    std::os::unix::fs::symlink(shared, cache.join("chrome")).unwrap();
}

/// This process's PATH with <bin> in front.
fn path_with(bin: &Path) -> std::ffi::OsString {
    let mut paths = vec![bin.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

/// The build: logs `start <label>`, starts a background sleep whose pid it
/// writes down in `background-<label>` (whole, through a rename, so a reader
/// never sees half of it), waits for the gate file `go-<label>` to appear,
/// then stops the sleep and logs `end <label>`. Stopped by SIGTERM it logs
/// `stop <label>`.
const BUILD: &str = r#"
label=$0; dir=$1
trap 'echo "stop $label" >> "$dir/log"; exit 1' TERM
echo "start $label" >> "$dir/log"
sleep 300 &
background=$!
# Until it has become the sleep: a signal before that is lost.
while [ "$(ps -o comm= -p $background)" != sleep ]; do :; done
echo $background > "$dir/background-$label.part"
mv "$dir/background-$label.part" "$dir/background-$label"
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
        let dir = test_dir(&format!("bq-{name}"));
        std::fs::create_dir_all(dir.join("volume/demo")).unwrap();
        std::fs::write(dir.join("volume/demo/app-mac-checkouts"), "mac-1\n").unwrap();
        let pinned_builds = programs_folder(&dir.join("home"));
        fake_apple_tools(&common::system(&dir), &dir.join("bin"));
        share_chrome(&dir.join("volume"));
        Volume {
            dir,
            pinned_builds,
            started: Vec::new(),
        }
    }

    fn builds(&self) -> Command {
        let mut command = common::builds(&self.dir);
        command
            .env("PATH", path_with(&self.dir.join("bin")))
            .env("QUEUE_TEST_DIR", &self.dir);
        command
    }

    /// Starts `builds lock demo` running the build as <label>; its pid.
    fn lock(&mut self, label: &str) -> i32 {
        self.lock_in(&["--lane", "mac"], label)
    }

    /// `lock` in the lane that <lane_args> (`--lane android`) names.
    fn lock_in(&mut self, lane_args: &[&str], label: &str) -> i32 {
        let child = Spawned::start(
            self.builds()
                .args(["lock", "demo"])
                .args(lane_args)
                .args(["sh", "-c", BUILD, label])
                .arg(&self.dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        );
        let pid = child.id() as i32;
        self.started.push(child);
        pid
    }

    /// Starts <args> as a `builds` command that is queued with the others;
    /// its pid.
    fn spawn(&mut self, args: &[&str]) -> i32 {
        let child = Spawned::start(
            self.builds()
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        );
        let pid = child.id() as i32;
        self.started.push(child);
        pid
    }

    /// A repo `demo` with an origin and a `.app-build` of the mac steps one and
    /// two (scripts that log like `BUILD` and wait for `go-one`, `go-two`),
    /// and its mac app checkout recorded.
    fn app_repo(&self) {
        self.app_repo_with("mac build ./one.sh\nmac app out.app\nmac bundle demo.test\nstep mac one ./one.sh\nstep mac two ./two.sh\n");
    }

    /// `app_repo` with <config> as its `.app-build`; besides `one.sh` and
    /// `two.sh`, `droid.sh` (a script like them), `bad.sh` (which logs and
    /// exits 3) and `dirty.sh` (which changes itself, a file the commit holds,
    /// adds the file `made` and exits 0), for the steps of <config> to name. The android lane's app
    /// checkout is recorded too, a worktree of the same repo.
    fn app_repo_with(&self, config: &str) {
        self.app_repo_at(&self.dir.join("checkout"), config);
    }

    fn app_repo_at(&self, top: &Path, config: &str) {
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(top)
                .args(args)
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        };
        std::fs::create_dir_all(top).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        let bad = top.join("bad.sh");
        std::fs::write(&bad, "#!/bin/sh\necho \"start bad\" >> \"$QUEUE_TEST_DIR/log\"\necho \"end bad\" >> \"$QUEUE_TEST_DIR/log\"\necho \"bad failed because reason-xyz\" >&2\nexit 3\n").unwrap();
        let mut mode = std::fs::metadata(&bad).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        std::fs::set_permissions(&bad, mode).unwrap();
        let dirty = top.join("dirty.sh");
        std::fs::write(
            &dirty,
            "#!/bin/sh\necho '# changed' >> \"$0\"\necho made > made\n",
        )
        .unwrap();
        let mut dirty_mode = std::fs::metadata(&dirty).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut dirty_mode, 0o755);
        std::fs::set_permissions(&dirty, dirty_mode).unwrap();
        for name in ["one", "two", "droid"] {
            let script = format!(
                "#!/bin/sh\ntrap 'echo \"stop {name}\" >> \"$QUEUE_TEST_DIR/log\"; exit 1' TERM\necho \"start {name}\" >> \"$QUEUE_TEST_DIR/log\"\nwhile [ ! -e \"$QUEUE_TEST_DIR/go-{name}\" ]; do sleep 0.05; done\necho \"end {name}\" >> \"$QUEUE_TEST_DIR/log\"\n"
            );
            let path = top.join(format!("{name}.sh"));
            std::fs::write(&path, script).unwrap();
            let mut mode = std::fs::metadata(&path).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
            std::fs::set_permissions(&path, mode).unwrap();
        }
        std::fs::write(top.join(".app-build"), config).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "Start"]);
        let origin = self.dir.join("origin.git");
        let made = Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&origin)
            .output()
            .unwrap();
        assert!(made.status.success());
        git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&["push", "-q", "origin", "main"]);
        std::fs::create_dir_all(self.dir.join("volume/demo/mac-1")).unwrap();
        std::fs::write(
            self.dir.join("volume/demo/mac-1/.checkout"),
            top.to_str().unwrap(),
        )
        .unwrap();
        let android = self.dir.join("checkout-android");
        git(&[
            "worktree",
            "add",
            "-q",
            "--detach",
            android.to_str().unwrap(),
        ]);
        std::fs::create_dir_all(self.dir.join("volume/demo/droid-1")).unwrap();
        std::fs::write(
            self.dir.join("volume/demo/droid-1/.checkout"),
            android.to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(
            self.dir.join("volume/demo/app-android-checkouts"),
            "droid-1\n",
        )
        .unwrap();
    }

    fn order(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("volume/demo/app-mac.queue/order"))
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn android_order(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("volume/demo/app-android.queue/order"))
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Waits until the order holds <count> tickets.
    fn until_queued(&self, count: usize) {
        until(&format!("{count} tickets in the order"), || {
            self.order().len() == count
        });
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
        until(&format!("{line:?} in the log"), || {
            self.log().iter().any(|l| l == line)
        });
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

    /// The pid of build <label>'s background sleep, once the build has
    /// written it: the build logs `start` before it starts the sleep.
    fn background(&self, label: &str) -> i32 {
        let file = self.dir.join(format!("background-{label}"));
        until(&format!("build {label}'s background pid"), || file.exists());
        std::fs::read_to_string(&file)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    /// Runs a `builds` command to its end.
    fn run(&self, args: &[&str]) -> std::process::Output {
        self.builds().args(args).output().unwrap()
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        self.started.clear();
        for label in LABELS {
            let text = std::fs::read_to_string(self.dir.join(format!("background-{label}")))
                .unwrap_or_default();
            if let Ok(pid) = text.trim().parse::<i32>() {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        // A step whose lane the test killed runs until its gate opens.
        for gate in LABELS.into_iter().chain(["one", "two", "droid"]) {
            self.open_gate(gate);
        }
        wait_for_free_lanes(&self.dir.join("volume/demo"));
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
    assert_eq!(
        log,
        lines(&[
            "start H", "end H", "start A", "end A", "start B", "end B", "start C", "end C"
        ])
    );
}

#[test]
fn top_moves_a_waiter_ahead_of_the_others() {
    let mut volume = Volume::new("top");
    let pids = queue_up(&mut volume, &["A", "B", "C"]);
    let moved = volume.run(&["queue", "demo", "top", &pids[3].to_string()]);
    assert!(
        moved.status.success(),
        "{}",
        String::from_utf8_lossy(&moved.stderr)
    );
    let shown = String::from_utf8(volume.run(&["queue", "demo"]).stdout).unwrap();
    let in_line = |pid: i32| {
        shown
            .find(&pid.to_string())
            .unwrap_or_else(|| panic!("{pid} not in:\n{shown}"))
    };
    assert!(
        in_line(pids[3]) < in_line(pids[1]) && in_line(pids[1]) < in_line(pids[2]),
        "next first:\n{shown}"
    );
    open_all_gates(&volume);
    let log = volume.log_of(8);
    assert_eq!(
        log,
        lines(&[
            "start H", "end H", "start C", "end C", "start A", "end A", "start B", "end B"
        ])
    );
}

#[test]
fn a_waiter_killed_in_line_lets_nobody_past_the_holder() {
    let mut volume = Volume::new("killed");
    let pids = queue_up(&mut volume, &["A", "B"]);
    unsafe { libc::kill(pids[1], libc::SIGKILL) };
    // B waited on A's ticket. A's death frees it, B takes it away and goes on
    // to wait on the holder's.
    until("A's ticket gone from the order", || {
        volume.order().len() == 2
    });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        volume.log(),
        lines(&["start H"]),
        "B started while H still held the lock"
    );
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
    assert!(
        taken.status.success(),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    // H's whole group was stopped, its background sleep too, before B starts.
    volume.until_logged("start B");
    assert!(
        !alive(background),
        "the stopped build's background process lives"
    );
    volume.until_logged("stop H");
    volume.open_gate("H");
    let log = volume.log_of(8);
    assert_eq!(
        log,
        lines(&[
            "start H", "stop H", "start B", "end B", "start H", "end H", "start A", "end A"
        ])
    );
    assert_one_at_a_time(&log);
}

#[test]
fn takeover_without_a_pid_takes_the_first_waiter() {
    let mut volume = Volume::new("takeover-first");
    queue_up(&mut volume, &["A", "B"]);
    volume.open_gate("A");
    volume.open_gate("B");
    let taken = volume.run(&["takeover", "demo"]);
    assert!(
        taken.status.success(),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    volume.until_logged("end A");
    volume.until_logged("stop H");
    volume.open_gate("H");
    let log = volume.log_of(8);
    assert_eq!(
        log,
        lines(&[
            "start H", "stop H", "start A", "end A", "start H", "end H", "start B", "end B"
        ])
    );
    assert_one_at_a_time(&log);
}

/// One request to the dashboard at <port>, answered with its status line.
fn request(port: u16, method: &str, path: &str, origin: Option<&str>, body: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let origin = origin
        .map(|origin| format!("Origin: {origin}\r\n"))
        .unwrap_or_default();
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
    pruned_now(&volume.dir.join("volume"));
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dashboard = Spawned::start(
        volume
            .builds()
            .args(["dashboard", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    );
    volume.started.push(dashboard);
    until("the dashboard listening", || {
        TcpStream::connect(("127.0.0.1", port)).is_ok()
    });
    let pids = queue_up(&mut volume, &["A", "B"]);
    let body = format!(r#"{{"action": "top", "repo": "demo", "pid": {}}}"#, pids[2]);
    let own = format!("http://127.0.0.1:{port}");

    assert!(
        request(port, "POST", "/api/queue", None, &body).contains("403"),
        "no Origin"
    );
    assert!(
        request(
            port,
            "POST",
            "/api/queue",
            Some("http://evil.example"),
            &body
        )
        .contains("403"),
        "another page"
    );
    assert!(
        request(port, "POST", "/api/other", Some(&own), &body).contains("405"),
        "another path"
    );
    assert!(
        request(port, "PUT", "/api/queue", Some(&own), &body).contains("405"),
        "another method"
    );
    let before = volume.order();
    assert_eq!(before.len(), 3);

    let moved = request(port, "POST", "/api/queue", Some(&own), &body);
    assert!(moved.contains("200"), "{moved}");
    let order = volume.order();
    assert_ne!(order, before, "B moved");
    assert_eq!(
        order[1].split('-').next().unwrap(),
        pids[2].to_string(),
        "B is behind the holder: {order:?}"
    );
    let bad = format!(
        r#"{{"action": "top", "repo": "../demo", "pid": {}}}"#,
        pids[1]
    );
    assert!(
        request(port, "POST", "/api/queue", Some(&own), &bad).contains("500"),
        "a path as the repo"
    );
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
        assert!(
            moved.status.success(),
            "{}",
            String::from_utf8_lossy(&moved.stderr)
        );
        volume.open_gate("A");
        volume.open_gate("B");
        volume.open_gate("X");
        volume.open_gate("H");
        // log_of bounds the wait.
        let log = volume.log_of(8);
        assert_eq!(
            log,
            lines(&[
                "start H", "end H", "start X", "end X", "start A", "end A", "start B", "end B"
            ]),
            "round {round}"
        );
    }
}

#[test]
fn a_takeover_runs_an_app_build_again_from_its_first_step() {
    let mut volume = Volume::new("app-build");
    volume.app_repo();
    volume.spawn(&["app-build", "demo", "main", "one", "two"]);
    volume.until_logged("start one");
    volume.lock("W");
    volume.until_queued(2);
    volume.open_gate("W");
    let taken = volume.run(&["takeover", "demo"]);
    assert!(
        taken.status.success(),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    volume.until_logged("end W");
    volume.until_logged("stop one");
    std::fs::write(volume.dir.join("go-one"), "").unwrap();
    std::fs::write(volume.dir.join("go-two"), "").unwrap();
    let log = volume.log_of(8);
    assert_eq!(
        log,
        lines(&[
            "start one",
            "stop one",
            "start W",
            "end W",
            "start one",
            "end one",
            "start two",
            "end two"
        ])
    );
    assert_one_at_a_time(&log);
}

/// An app build holds no use of the stores while it waits for the lane or
/// runs a step. It runs each step once and the installed builds saves the
/// passing run. The fixture changes the install generation and puts builds
/// on PATH under the scratch home.
#[test]
fn an_install_leaves_a_waiting_or_running_app_build_to_build_once() {
    use std::os::fd::AsRawFd;
    let mut volume = Volume::new("app-build-install");
    volume.app_repo();
    let project = volume.run(&["lists", "init", "demo", "--repos", "demo", "--by", "test"]);
    assert!(
        project.status.success(),
        "{}",
        String::from_utf8_lossy(&project.stderr)
    );
    volume.lock("H");
    volume.until_logged("start H");
    let build = start_app_build(&volume, &["one", "two"]);
    volume.until_queued(2);
    let install = volume
        .dir
        .join("home/Library/Application Support/builds/install");
    let uses = |kind: &str| match std::fs::read_dir(install.join(kind)) {
        Ok(entries) => entries.count(),
        Err(_) => 0,
    };
    assert_eq!(uses("commands"), 0, "the waiting build holds no use");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("generation"), "1\n").unwrap();
    volume.open_gate("H");
    volume.until_logged("start one");
    assert_eq!(uses("commands"), 0, "the building lane holds no use");
    let gate = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(install.join("commands.gate"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(gate.as_raw_fd(), libc::LOCK_EX) }, 0);
    std::fs::write(install.join("generation"), "2\n").unwrap();
    let bin = volume.dir.join("home/.local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_builds"), bin.join("builds")).unwrap();
    drop(gate);
    volume.open_gate("one");
    volume.open_gate("two");
    let (status, said) = finish(build);
    assert!(status.success(), "{said}");
    assert!(!said.contains("installed builds"), "{said}");
    let store = install.with_file_name("lists").join("demo/builds.sqlite");
    let conn = rusqlite::Connection::open(store).unwrap();
    let count: u32 = conn
        .query_row("SELECT COUNT(*) FROM app_build_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "the passing build leaves one record");
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM app_build_runs r JOIN app_build_run_commit c ON c.run_id = r.id WHERE r.built_ref = 'main' AND r.outcome = 'passed'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(
        count, 1,
        "the record keeps the passing build's ref and commit"
    );
    assert_eq!(
        volume.log(),
        lines(&[
            "start H",
            "end H",
            "start one",
            "end one",
            "start two",
            "end two"
        ])
    );
}

#[test]
fn a_lane_refuses_to_start_if_install_moved_the_store_after_program_selection() {
    use std::os::unix::fs::PermissionsExt;

    let volume = Volume::new("lane-install");
    volume.app_repo();
    volume.open_gate("one");
    let project = volume.run(&["lists", "init", "demo", "--repos", "demo", "--by", "test"]);
    assert!(
        project.status.success(),
        "{}",
        String::from_utf8_lossy(&project.stderr)
    );

    let support = volume.dir.join("home/Library/Application Support/builds");
    let store = support.join("lists/demo/builds.sqlite");
    let moved = volume.dir.join("moved.sqlite");
    std::fs::copy(&store, &moved).unwrap();
    let conn = rusqlite::Connection::open(&moved).unwrap();
    let stamp: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    conn.execute_batch("CREATE TABLE new_install (id INTEGER PRIMARY KEY);")
        .unwrap();
    conn.pragma_update(None, "user_version", stamp ^ 1).unwrap();
    drop(conn);

    // The parent selects this path. The wrapper finishes the store move
    // before it starts the old lane executable.
    let pinned = &volume.pinned_builds;
    std::fs::remove_file(pinned).unwrap();
    std::fs::write(
        pinned,
        r#"#!/bin/sh
set -e
support="$HOME/Library/Application Support/builds"
mv "$QUEUE_TEST_DIR/moved.sqlite" "$support/lists/demo/builds.sqlite"
echo 1 > "$support/install/generation"
exec "$OLD_BUILDS" "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(pinned, std::fs::Permissions::from_mode(0o755)).unwrap();

    let ran = volume
        .builds()
        .env("OLD_BUILDS", env!("CARGO_BIN_EXE_builds"))
        .args(["app-build", "demo", "main", "one"])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(!ran.status.success(), "{said}");
    assert!(
        said.contains("builds install ran before the app build lane started"),
        "{said}"
    );
    assert!(volume.log().is_empty(), "{said}");
}

#[test]
fn app_build_runs_the_steps_named_and_lists_them() {
    let volume = Volume::new("app-build-steps");
    volume.app_repo();
    let listed = volume.run(&["app-build", "demo", "--list"]);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "mac one ./one.sh\nmac two ./two.sh\n"
    );
    std::fs::write(volume.dir.join("go-two"), "").unwrap();
    let built = volume.run(&["app-build", "demo", "main", "two"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert_eq!(volume.log(), lines(&["start two", "end two"]));
    let unknown = volume.run(&["app-build", "demo", "main", "three"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("no step three"));
}

/// A step's `builds cargo` runs in the app checkout, which no claim names a
/// project for, so in a repo two projects hold it records in the project of
/// the app build.
#[test]
fn a_steps_cargo_run_records_in_the_app_builds_project() {
    use std::os::unix::fs::PermissionsExt;

    let volume = Volume::new("app-build-cargo-project");
    volume.app_repo_at(
        &volume.dir.join("home/dev/demo"),
        "mac build ./one.sh\nmac app out.app\nmac bundle demo.test\nstep mac compile builds cargo check\n",
    );
    let cargo = volume.dir.join("bin/cargo");
    std::fs::write(&cargo, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    for project in ["one", "two"] {
        let made = volume.run(&["lists", "init", project, "--repos", "demo", "--by", "test"]);
        assert!(made.status.success(), "{made:?}");
    }
    let built = volume.run(&["app-build", "demo", "main", "compile", "--project", "two"]);
    let said = String::from_utf8_lossy(&built.stderr);
    assert!(built.status.success(), "{said}");
    let runs = |project: &str| -> u32 {
        let store = volume
            .dir
            .join("home/Library/Application Support/builds/lists")
            .join(project)
            .join("builds.sqlite");
        rusqlite::Connection::open(store)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM builds_cargo_runs", [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!((runs("one"), runs("two")), (0, 1), "{said}");
}

#[test]
fn a_step_that_changes_the_checkout_fails_the_build_which_puts_the_checkout_back() {
    let volume = Volume::new("app-build-dirty");
    volume.app_repo_with("step mac dirty ./dirty.sh\nstep mac two ./two.sh\n");
    let top = volume.dir.join("checkout");
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&top)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let head = git(&["rev-parse", "HEAD"]);
    let built = volume.run(&["app-build", "demo", "main", "dirty"]);
    let said = String::from_utf8_lossy(&built.stderr);
    assert!(!built.status.success(), "the build fails: {said}");
    assert!(
        said.contains("dirty.sh") && said.contains("made"),
        "the build names the files its steps changed: {said}"
    );
    assert_eq!(git(&["status", "--porcelain"]), "", "the checkout is clean");
    assert_eq!(git(&["rev-parse", "HEAD"]), head, "on its commit");
    volume.open_gate("two");
    let next = volume.run(&["app-build", "demo", "main", "two"]);
    assert!(
        next.status.success(),
        "the next build takes the checkout: {}",
        String::from_utf8_lossy(&next.stderr)
    );
}

#[test]
fn app_build_refuses_a_ci_only_step_in_each_form_and_lists_it_as_ci_only() {
    let volume = Volume::new("app-build-ci-only");
    volume.app_repo_with("step mac one ./one.sh\nstep mac two ./two.sh\nci-only two\n");
    let listed = volume.run(&["app-build", "demo", "--list"]);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "mac one ./one.sh\nmac two ./two.sh (CI only)\n"
    );
    std::fs::write(volume.dir.join("go-two"), "").unwrap();
    for form in [
        &["app-build", "demo", "main", "two"][..],
        &[
            "app-build",
            "demo",
            "--lane",
            "mac",
            "main",
            "--built-ref",
            "main",
            "two",
        ][..],
    ] {
        let refused = volume.run(form);
        assert!(!refused.status.success(), "{form:?} ran");
        let said = String::from_utf8_lossy(&refused.stderr);
        assert!(
            said.contains("step two") && said.contains("CI runs the full suites"),
            "{said}"
        );
    }
    assert!(volume.log().is_empty(), "{:?}", volume.log());
}

#[test]
fn a_step_past_its_limit_fails_the_build_and_the_lock_goes_to_the_next_in_line() {
    let mut volume = Volume::new("limit");
    volume.app_repo_with("step mac one ./one.sh\nlimit one 1s\n");
    let mut build = Spawned::start(
        volume
            .builds()
            .args(["app-build", "demo", "main", "one"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    );
    volume.until_logged("start one");
    volume.lock("W");
    volume.until_queued(2);
    volume.open_gate("W");
    // The step never gets its gate: only the limit ends it.
    volume.until_logged("end W");
    let log = volume.log();
    assert_eq!(log, lines(&["start one", "stop one", "start W", "end W"]));
    let mut said = String::new();
    std::io::Read::read_to_string(build.stderr.as_mut().unwrap(), &mut said).unwrap();
    let status = build.wait().unwrap();
    assert!(!status.success(), "the build fails");
    assert!(
        said.contains("step one") && said.contains("limit of 1s"),
        "{said}"
    );
}

const LANES: &str = "step mac one ./one.sh\nstep mac two ./two.sh\nstep android droid ./droid.sh\n";

#[test]
fn a_killed_android_lane_keeps_its_checkout_until_the_running_step_ends() {
    let mut volume = Volume::new("lane-killed");
    volume.app_repo_with(LANES);
    volume.spawn(&[
        "app-build",
        "demo",
        "--lane",
        "android",
        "main",
        "--built-ref",
        "main",
        "droid",
    ]);
    volume.until_logged("start droid");
    let lane = volume.started.last_mut().unwrap();
    lane.kill().unwrap();
    assert!(!lane.wait().unwrap().success());

    let during = volume.run(&["queue", "demo", "--lane", "android"]);
    volume.open_gate("droid");
    volume.until_logged("end droid");
    let said = String::from_utf8(during.stdout).unwrap();
    assert!(
        during.status.success() && said.contains("android droid-1: held"),
        "{said}"
    );
    let next = volume.run(&["lock", "demo", "--lane", "android", "true"]);
    assert!(
        next.status.success(),
        "{}",
        String::from_utf8_lossy(&next.stderr)
    );
}

/// Starts `builds app-build demo main <steps>`, its output piped.
fn start_app_build(volume: &Volume, steps: &[&str]) -> Spawned {
    Spawned::start(
        volume
            .builds()
            .args(["app-build", "demo", "main"])
            .args(steps)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
}

/// What the build said, its exit status after.
fn finish(mut build: Spawned) -> (std::process::ExitStatus, String) {
    let mut said = String::new();
    std::io::Read::read_to_string(build.stdout.as_mut().unwrap(), &mut said).unwrap();
    let mut errors = String::new();
    std::io::Read::read_to_string(build.stderr.as_mut().unwrap(), &mut errors).unwrap();
    let status = build.wait().unwrap();
    (status, said + &errors)
}

#[test]
fn an_app_build_runs_the_lanes_at_the_same_time_each_in_its_own_checkout() {
    let volume = Volume::new("two-lanes");
    volume.app_repo_with(LANES);
    let build = start_app_build(&volume, &["one", "two", "droid"]);
    // Both lanes' first steps run before either is let go.
    volume.until_logged("start one");
    volume.until_logged("start droid");
    assert_eq!(volume.log().len(), 2, "{:?}", volume.log());
    for gate in ["one", "two", "droid"] {
        volume.open_gate(gate);
    }
    let (status, said) = finish(build);
    assert!(status.success(), "{said}");
    let log = volume.log();
    let at = |line: &str| {
        log.iter()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("{line} not in {log:?}"))
    };
    assert!(
        at("end one") < at("start two"),
        "a lane's steps run in order: {log:?}"
    );
    assert!(
        at("start droid") < at("end one"),
        "the lanes ran at once: {log:?}"
    );
    assert!(
        said.contains("[mac mac-1] builds: step one")
            && said.contains("[android droid-1] builds: step droid"),
        "{said}"
    );
    // Each lane's steps ran in its own checkout.
    let head = |dir: &str| {
        let out = Command::new("git")
            .arg("-C")
            .arg(volume.dir.join(dir))
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(head("checkout"), head("checkout-android"));
}

#[test]
fn a_lane_waits_for_its_own_lock_only() {
    let mut volume = Volume::new("lane-locks");
    volume.app_repo_with(LANES);
    // The mac lane is held and stays held.
    volume.lock("H");
    volume.until_logged("start H");
    volume.open_gate("droid");
    let (status, said) = finish(start_app_build(&volume, &["droid"]));
    assert!(status.success(), "{said}");
    assert_eq!(
        volume.log(),
        lines(&["start H", "start droid", "end droid"]),
        "the android build did not wait for the mac lane"
    );
    // A waiter in the mac lane's line is not in the android lane's.
    let waiter = volume.lock("A");
    volume.until_queued(2);
    let in_android = |volume: &Volume| -> Vec<String> {
        volume
            .android_order()
            .iter()
            .map(|name| name.split('-').next().unwrap().to_string())
            .collect()
    };
    assert!(
        !in_android(&volume).contains(&waiter.to_string()),
        "A waits in the mac lane's line only"
    );
    // `--lane android` takes the other lock, free now.
    let other = volume.lock_in(&["--lane", "android"], "B");
    volume.until_logged("start B");
    assert_eq!(in_android(&volume), vec![other.to_string()]);
    assert_eq!(volume.order().len(), 2, "the mac lane's line is as it was");
}

#[test]
fn a_build_failing_in_one_lane_fails_the_build_naming_the_lane_and_the_other_lane_finishes() {
    let volume = Volume::new("lane-fails");
    volume.app_repo_with("step mac bad ./bad.sh\nstep android droid ./droid.sh\n");
    let build = start_app_build(&volume, &["bad", "droid"]);
    volume.until_logged("end bad");
    volume.until_logged("start droid");
    volume.open_gate("droid");
    let (status, said) = finish(build);
    assert_eq!(status.code(), Some(1), "{said}");
    assert!(
        said.contains("app-build failed in the mac lane (exit 3;"),
        "{said}"
    );
    assert!(!said.contains("android lane (exit"), "{said}");
    // The failing lane's last lines come back at the end, after the other
    // lane's output, where the reader looks.
    let end = said.rfind("the last").expect("the tail of the failed lane");
    assert!(
        said[end..].contains("of the mac lane:") && said[end..].contains("reason-xyz"),
        "{said}"
    );
    assert!(
        volume.log().contains(&"end droid".to_string()),
        "the android lane ran to its end: {:?}",
        volume.log()
    );
}

#[test]
fn a_step_is_refused_by_the_lane_it_is_not_in() {
    let volume = Volume::new("lane-refuses");
    volume.app_repo_with(LANES);
    let refused = volume.run(&[
        "app-build",
        "demo",
        "--lane",
        "mac",
        "main",
        "--built-ref",
        "main",
        "droid",
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("step droid is in the android lane, not the mac lane")
    );
    assert!(volume.log().is_empty());
}

#[test]
fn a_pid_names_its_lane_for_a_takeover_and_queue_shows_both_lanes() {
    let mut volume = Volume::new("lane-takeover");
    volume.app_repo();
    volume.lock_in(&["--lane", "android"], "H");
    volume.until_logged("start H");
    let waiter = volume.lock_in(&["--lane", "android"], "A");
    until("A in the android lane's line", || {
        volume.android_order().len() == 2
    });
    volume.open_gate("A");
    let shown = String::from_utf8(volume.run(&["queue", "demo"]).stdout).unwrap();
    assert!(
        shown.contains("android droid-1: holder")
            && shown.contains(&format!("android: 1. {waiter}")),
        "{shown}"
    );
    assert!(shown.contains("mac: nobody waits"), "{shown}");
    // No --lane: the pid says which lane's holder to stop.
    let taken = volume.run(&["takeover", "demo", &waiter.to_string()]);
    assert!(
        taken.status.success(),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    volume.until_logged("end A");
    volume.until_logged("stop H");
    volume.open_gate("H");
    let log = volume.log_of(6);
    assert_eq!(
        log,
        lines(&["start H", "stop H", "start A", "end A", "start H", "end H"])
    );
}

#[test]
fn a_lane_with_two_checkouts_builds_in_both_at_once_and_the_next_in_line_waits_for_either() {
    let mut volume = Volume::new("lane-set");
    volume.app_repo();
    std::fs::write(
        volume.dir.join("volume/demo/app-android-checkouts"),
        "droid-1\ndroid-2\n",
    )
    .unwrap();
    volume.lock_in(&["--lane", "android"], "A");
    volume.lock_in(&["--lane", "android"], "B");
    volume.until_logged("start A");
    volume.until_logged("start B");
    let third = volume.lock_in(&["--lane", "android"], "C");
    until("C in the android lane's line", || {
        volume.android_order().len() == 3
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        volume.log().len(),
        2,
        "both locks are held: {:?}",
        volume.log()
    );

    let shown =
        String::from_utf8(volume.run(&["queue", "demo", "--lane", "android"]).stdout).unwrap();
    assert!(
        shown.contains("android droid-1: holder") && shown.contains("android droid-2: holder"),
        "{shown}"
    );
    assert!(
        shown.contains(&format!("android: 1. {third}")),
        "only C waits: {shown}"
    );

    // B, the later holder, ends: C takes the lock B had, while A still runs.
    volume.open_gate("B");
    volume.until_logged("end B");
    volume.until_logged("start C");
    assert!(!volume.log().contains(&"end A".to_string()));
    volume.open_gate("A");
    volume.open_gate("C");
    volume.until_logged("end C");
}

#[test]
fn a_takeover_with_every_lock_held_stops_the_last_holder_and_a_free_lock_just_moves_the_waiter() {
    let mut volume = Volume::new("lane-set-takeover");
    volume.app_repo();
    std::fs::write(
        volume.dir.join("volume/demo/app-android-checkouts"),
        "droid-1\ndroid-2\n",
    )
    .unwrap();
    volume.lock_in(&["--lane", "android"], "A");
    volume.until_logged("start A");
    volume.lock_in(&["--lane", "android"], "B");
    volume.until_logged("start B");
    let waiter = volume.lock_in(&["--lane", "android"], "X");
    until("X in the line", || volume.android_order().len() == 3);
    volume.open_gate("X");
    let taken = volume.run(&["takeover", "demo", "--lane", "android", &waiter.to_string()]);
    assert!(
        taken.status.success(),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    // B joined last of the holders: it stops, X runs, B runs again after X.
    volume.until_logged("stop B");
    volume.until_logged("end X");
    assert!(
        !volume.log().contains(&"stop A".to_string()),
        "A kept running: {:?}",
        volume.log()
    );
    until("B started again", || {
        volume.log().iter().filter(|l| *l == "start B").count() == 2
    });
    volume.open_gate("A");
    volume.open_gate("B");
    volume.until_logged("end B");
}

#[test]
fn a_class_only_run_tests_the_commit_the_checkout_last_built() {
    let volume = Volume::new("class-built");
    volume
        .app_repo_with("step mac one ./one.sh\nstep mac two ./two.sh\ntest mac ./one.sh {class}\n");
    let said = |output: &std::process::Output| String::from_utf8_lossy(&output.stderr).into_owned();
    // Nothing built yet.
    let none = volume.run(&[
        "app-build",
        "demo",
        "--lane",
        "mac",
        "main",
        "--built-ref",
        "main",
        "--test",
        "mac",
        "Foo",
    ]);
    assert!(
        !none.status.success() && said(&none).contains("no finished build"),
        "{}",
        said(&none)
    );
    assert!(volume.log().is_empty(), "the class ran: {:?}", volume.log());
    // Build, then test the same commit.
    std::fs::write(volume.dir.join("go-two"), "").unwrap();
    let built = volume.run(&["app-build", "demo", "main", "two"]);
    assert!(built.status.success(), "{}", said(&built));
    std::fs::write(volume.dir.join("go-one"), "").unwrap();
    let tested = volume.run(&[
        "app-build",
        "demo",
        "--lane",
        "mac",
        "main",
        "--built-ref",
        "main",
        "--test",
        "mac",
        "Foo",
    ]);
    assert!(tested.status.success(), "{}", said(&tested));
    assert_eq!(
        volume.log(),
        lines(&["start two", "end two", "start one", "end one"])
    );
    // Another commit than the one built is refused, naming what was built.
    let top = volume.dir.join("checkout");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&top)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let built_commit = git(&["rev-parse", "HEAD"]);
    let other = git(&["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "Other"]);
    git(&["push", "-q", "origin", &format!("{other}:refs/heads/other")]);
    let refused = volume.run(&[
        "app-build",
        "demo",
        "--lane",
        "mac",
        "origin/other",
        "--built-ref",
        "origin/other",
        "--test",
        "mac",
        "Foo",
    ]);
    let text = said(&refused);
    assert!(
        !refused.status.success() && text.contains(&built_commit) && text.contains("two"),
        "{text}"
    );
    assert_eq!(
        volume.log().len(),
        4,
        "the class ran against the stale build"
    );
}

#[test]
fn a_shots_only_run_tests_the_commit_the_checkout_last_built() {
    let volume = Volume::new("shots-built");
    // The android lane has a step and no shots line.
    volume.app_repo_with(&format!("{SHOTS_CONFIG}step android droid ./droid.sh\n"));
    commit_shots(&volume.dir.join("checkout"), "");
    let said = |output: &std::process::Output| String::from_utf8_lossy(&output.stderr).into_owned();
    let none = volume.run(&[
        "app-build",
        "demo",
        "--lane",
        "mac",
        "main",
        "--built-ref",
        "main",
        "--shots",
        "mac",
        "mac.a.",
    ]);
    assert!(
        !none.status.success() && said(&none).contains("no finished build"),
        "{}",
        said(&none)
    );
    assert!(volume.log().is_empty(), "the shots ran: {:?}", volume.log());
    volume.open_gate("one");
    let built = volume.run(&["app-build", "demo", "main", "one"]);
    assert!(built.status.success(), "{}", said(&built));
    let shot = volume.run(&[
        "app-build",
        "demo",
        "--lane",
        "mac",
        "main",
        "--built-ref",
        "main",
        "--shots",
        "mac",
        "mac.a.",
    ]);
    assert!(shot.status.success(), "{}", said(&shot));
    let log = volume.log();
    assert_eq!(log.len(), 3, "{log:?}");
    assert_eq!(log[..2], lines(&["start one", "end one"]));
    assert_eq!(shots_runs(&volume, "mac.a.").len(), 1, "{log:?}");
    let kept =
        std::fs::read_to_string(volume.dir.join("volume/demo/mac-1/.last-app-build")).unwrap();
    assert_eq!(
        kept.lines().nth(1),
        Some("one"),
        "a shots run leaves what was built as it was: {kept:?}"
    );
    let missing = volume.run(&[
        "app-build",
        "demo",
        "--lane",
        "android",
        "main",
        "--built-ref",
        "main",
        "--shots",
        "android",
        "android.a.",
    ]);
    assert!(
        !missing.status.success() && said(&missing).contains("no `shots android ...` line"),
        "{}",
        said(&missing)
    );
}

/// With no step named, a branch that changes both lanes' folders builds both
/// lanes' steps, and each test class and shots prefix goes to its lane. The
/// mac lane's two classes run in one run of its test line.
#[test]
fn app_build_with_no_step_named_builds_both_lanes_and_runs_each_class_and_shots_in_its_lane() {
    let volume = Volume::new("derive-both");
    volume.app_repo_with(
        "step mac one ./one.sh\nstep android droid ./droid.sh\n\
         on mac/ one\non droid/ droid\n\
         sources mac mac\nsources android droid\n\
         needs mac one\nneeds android droid\n\
         test mac ./one.sh {class}\ntest android ./droid.sh {class}\n\
         shots mac ./shot.sh --select {select}\nshots android ./shot.sh --select {select}\n\
         shots-declared mac declared-mac\nshots-declared android declared-android\n",
    );
    let top = volume.dir.join("checkout");
    reporting_shots(&top);
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&top)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["checkout", "-q", "-b", "work"]);
    std::fs::create_dir_all(top.join("mac")).unwrap();
    std::fs::create_dir_all(top.join("droid")).unwrap();
    std::fs::write(top.join("mac/MacFoo.txt"), "struct MacFoo {}\n").unwrap();
    std::fs::write(top.join("mac/MacBar.txt"), "struct MacBar {}\n").unwrap();
    std::fs::write(top.join("droid/DroidFoo.txt"), "object DroidFoo {}\n").unwrap();
    git(&["add", "mac", "droid"]);
    git(&["commit", "-q", "-m", "Change both lanes"]);
    git(&["push", "-q", "origin", "work"]);
    git(&["checkout", "-q", "main"]);
    volume.open_gate("one");
    volume.open_gate("droid");
    let built = volume.run(&[
        "app-build",
        "demo",
        "work",
        "--test",
        "MacFoo",
        "--test",
        "DroidFoo",
        "--test",
        "MacBar",
        "--shots",
        "android.a.",
        "--shots",
        "mac.a.",
    ]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(built.status.success(), "{said}");
    let log = volume.log();
    let count = |line: &str| log.iter().filter(|l| *l == line).count();
    // The merge base's `needs` step, then the branch's step and the run of
    // the test line, in each lane.
    assert_eq!(count("end one"), 3, "{log:?}\n{said}");
    assert_eq!(count("end droid"), 3, "{log:?}\n{said}");
    // Each lane's shots run, first at the merge base, then on the branch,
    // comparing with the merge base's run.
    for prefix in ["mac.a.", "android.a."] {
        let runs: Vec<&String> = log
            .iter()
            .filter(|line| line.starts_with(&format!("shots --select {prefix} ")))
            .collect();
        assert_eq!(runs.len(), 2, "{log:?}\n{said}");
        assert!(!runs[0].contains("--before"), "{log:?}");
        assert!(runs[0].contains("--base"), "{log:?}");
        assert!(runs[1].contains("--before"), "{log:?}");
        assert!(!runs[1].contains("--base"), "{log:?}");
    }
}

/// Writes <file> with <text> in the declare folder <folder> of <top>, which
/// the `shots-declared` lines of `.app-build` name.
fn declare_scenes(top: &Path, folder: &str, file: &str, text: &str) {
    std::fs::create_dir_all(top.join(folder)).unwrap();
    std::fs::write(top.join(folder).join(file), text).unwrap();
}

/// Commits the reporting shots program and declares one scene on both lanes.
fn reporting_shots(top: &Path) {
    write_reporting_shots(top, "");
    declare_scenes(
        top,
        "declared-android",
        "Shots.kt",
        "val scenes = listOf(shot(\"android.a.one\"))\n",
    );
    git_in(top, &["add", "shot.sh", "declared-mac", "declared-android"]);
    git_in(top, &["commit", "-q", "-m", "Add the shots program"]);
    git_in(top, &["push", "-q", "origin", "main"]);
}

/// `--all-shots mac` runs the mac lane's shots line once, with the prefix
/// every mac shot starts with, after the steps the lane needs; `--shots mac.`
/// stays refused and names `--all-shots mac`.
#[test]
fn all_shots_runs_the_lanes_shots_line_for_every_shot_of_the_lane() {
    let volume = Volume::new("all-shots");
    volume.app_repo_with(SHOTS_CONFIG);
    commit_shots(&volume.dir.join("checkout"), "");
    let refused = volume.run(&["app-build", "demo", "main", "--shots", "mac."]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success() && said.contains("--all-shots mac"),
        "{said}"
    );
    assert!(volume.log().is_empty(), "{:?}", volume.log());
    volume.open_gate("one");
    let built = volume.run(&["app-build", "demo", "main", "--all-shots", "mac"]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(built.status.success(), "{said}");
    let log = volume.log();
    assert_eq!(log[..2], lines(&["start one", "end one"]), "{said}");
    assert_eq!(log.len(), 3, "{log:?}");
    assert!(
        log[2].starts_with("shots --select mac. --pictures "),
        "{log:?}"
    );
}

/// The shots step of an app build runs with TMPDIR on the build volume, in a
/// folder of the run that the end of the build deletes, so the Chrome that the
/// shots checker starts writes its profile there and not where fseventsd logs
/// it.
#[test]
fn a_shots_step_sees_a_tmpdir_on_the_build_volume() {
    let volume = Volume::new("shots-tmpdir");
    volume.app_repo_with(SHOTS_CONFIG);
    commit_shots(
        &volume.dir.join("checkout"),
        r#"set -e
echo "$TMPDIR" > "$QUEUE_TEST_DIR/shots-tmpdir"
echo written > "$TMPDIR/profile""#,
    );
    volume.open_gate("one");
    let built = volume.run(&["app-build", "demo", "main", "--all-shots", "mac"]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(built.status.success(), "{said}");
    let seen = PathBuf::from(
        std::fs::read_to_string(volume.dir.join("shots-tmpdir"))
            .unwrap()
            .trim(),
    );
    assert_eq!(
        seen.parent().unwrap(),
        volume.dir.join("volume/.tmp"),
        "{said}"
    );
    assert!(!seen.exists(), "the build's folder is still there: {said}");
}

#[test]
fn queue_shows_every_checkout_and_why_builds_added_or_removed_one() {
    let volume = Volume::new("lane-why");
    std::fs::write(
        volume.dir.join("volume/demo/app-mac-checkouts"),
        "mac-1\nm1:builds-mac-1 builds 1763337600 2 builds waited for the mac lane's 1 checkout\n",
    )
    .unwrap();
    std::fs::write(
        volume.dir.join("volume/demo/app-mac-checkouts-removed"),
        "1763341200 m1:builds-mac-2 no build used it for 61 minutes\n",
    )
    .unwrap();
    let shown = String::from_utf8(volume.run(&["queue", "demo"]).stdout).unwrap();
    assert!(
        shown.contains("mac mac-1: free\n") && shown.contains("mac m1:builds-mac-1: free\n"),
        "{shown}"
    );
    assert!(
        shown.contains("mac m1:builds-mac-1: builds added it at ")
            && shown.contains(": 2 builds waited for the mac lane's 1 checkout\n"),
        "{shown}"
    );
    assert!(
        shown.contains("mac: builds removed m1:builds-mac-2 at ")
            && shown.contains(": no build used it for 61 minutes\n"),
        "{shown}"
    );
}

/// A takeover stops the holder H, which waits in the queue to run again. A
/// stop signal then ends H's wait: H exits instead of running its build
/// again, and the build that took over runs on.
#[test]
fn a_stop_signal_ends_a_holder_that_waits_to_run_again_after_a_takeover() {
    let mut volume = Volume::new("stop-requeued");
    let pids = queue_up(&mut volume, &["A"]);
    let taken = volume.run(&["takeover", "demo", &pids[1].to_string()]);
    assert!(
        taken.status.success(),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    volume.until_logged("start A");
    volume.until_logged("stop H");
    unsafe { libc::kill(pids[0], libc::SIGTERM) };
    let at = volume
        .started
        .iter()
        .position(|child| child.id() as i32 == pids[0])
        .unwrap();
    let mut holder = volume.started.remove(at);
    let (sent, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sent.send(holder.wait().unwrap());
    });
    let status = ended
        .recv_timeout(Duration::from_secs(20))
        .expect("the holder ignored SIGTERM while it waited to run again");
    assert!(!status.success());
    volume.open_gate("A");
    volume.open_gate("H");
    volume.until_logged("end A");
    assert_eq!(
        volume.log(),
        lines(&["start H", "stop H", "start A", "end A"]),
        "H never ran again"
    );
}

/// A `.app-build` of the mac lane with a build step, a shots line and the
/// folder that declares the shots.
const SHOTS_CONFIG: &str = "step mac one ./one.sh\non mac/ one\nneeds mac one\nshots mac ./shot.sh --select {select}\nshots-declared mac declared-mac\n";

/// What `git <args>` printed in <top>.
fn git_in(top: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(top)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Writes a shots program that logs its arguments, runs <hook> and writes
/// a report in the fixture's folder. Declares one Mac scene in the checkout.
fn write_reporting_shots(top: &Path, hook: &str) {
    let shot = top.join("shot.sh");
    assert!(!shot.exists(), "shot.sh exists already");
    std::fs::write(
        &shot,
        format!(
            "#!/bin/sh
echo \"shots $*\" >> \"$QUEUE_TEST_DIR/log\"
{hook}
report=\"$QUEUE_TEST_DIR/report-$$\"
mkdir -p \"$report\"
echo '<html></html>' > \"$report/index.html\"
echo '{{\"version\": \"{}\", \"capture_version\": \"{}\", \"commit\": \"c\", \"scenes\": [], \"boards\": {{}}, \"findings\": [], \"failed\": [], \"problems\": 0}}' > \"$report/findings.json\"
echo \"builds shots: report at $report/index.html\" >&2
",
            env!("COMPARISON_SOURCE"),
            env!("CAPTURE_SOURCE")
        ),
    )
    .unwrap();
    let mut mode = std::fs::metadata(&shot).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&shot, mode).unwrap();
    declare_scenes(
        top,
        "declared-mac",
        "Shots.swift",
        "let scenes = [\"mac.a.one\"]\n",
    );
}

/// Commits the reporting shots program and the declared Mac scene on main,
/// and returns main's commit.
fn commit_shots(top: &Path, hook: &str) -> String {
    write_reporting_shots(top, hook);
    git_in(top, &["add", "shot.sh", "declared-mac"]);
    git_in(top, &["commit", "-q", "-m", "Add the shots program"]);
    git_in(top, &["push", "-q", "origin", "main"]);
    git_in(top, &["rev-parse", "HEAD"])
}

/// Makes the branch <name> from main's commit, with a commit that changes
/// <file> (a path of the mac lane's app), pushes it and checks main out
/// again.
fn branch_from_main(top: &Path, name: &str, file: &str) {
    git_in(top, &["checkout", "-q", "-b", name]);
    std::fs::create_dir_all(top.join("mac")).unwrap();
    std::fs::write(top.join(file), format!("{name}\n")).unwrap();
    git_in(top, &["add", file]);
    git_in(
        top,
        &["commit", "-q", "-m", &format!("Change the app in {name}")],
    );
    git_in(top, &["push", "-q", "origin", name]);
    git_in(top, &["checkout", "-q", "main"]);
}

/// The lines of the log that are runs of the shots program for <prefix>.
fn shots_runs(volume: &Volume, prefix: &str) -> Vec<String> {
    volume
        .log()
        .into_iter()
        .filter(|line| line.starts_with(&format!("shots --select {prefix} ")))
        .collect()
}

fn cached_coverage_repo(name: &str, shot_hook: &str) -> (Volume, PathBuf) {
    let volume = Volume::new(name);
    let top = volume.dir.join("demo");
    volume.app_repo_at(
        &top,
        &format!("{SHOTS_CONFIG}step android droid ./droid.sh\non android/ droid\n"),
    );
    let project = volume.run(&["lists", "init", "demo", "--repos", "demo", "--by", "test"]);
    assert!(
        project.status.success(),
        "{}",
        String::from_utf8_lossy(&project.stderr)
    );
    commit_shots(&top, shot_hook);
    branch_from_main(&top, "work", "mac/Foo.txt");
    git_in(&top, &["checkout", "-q", "work"]);
    declare_scenes(
        &top,
        "declared-mac",
        "Shots.swift",
        "let scenes = [\"mac.a.one\", \"mac.b.one\"]\n",
    );
    git_in(&top, &["add", "declared-mac"]);
    git_in(&top, &["commit", "-q", "-m", "Declare a new shot"]);
    git_in(&top, &["push", "-q", "origin", "work"]);
    volume.open_gate("one");
    (volume, top)
}

fn run_with_shots(volume: &Volume, prefixes: &[&str]) {
    let mut args = vec!["app-build", "demo", "work"];
    for prefix in prefixes {
        args.extend(["--shots", prefix]);
    }
    let built = volume.run(&args);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
}

fn assert_landing_coverage(volume: &Volume, top: &Path) {
    let head = git_in(top, &["rev-parse", "work"]);
    let own = git_in(top, &["rev-list", "--reverse", "origin/main..work"]);
    let mut args = vec![
        "app-build",
        "--landing-coverage",
        top.to_str().unwrap(),
        &head,
        "work",
    ];
    args.extend(own.lines());
    let coverage = volume.run(&args);
    assert!(
        coverage.status.success(),
        "{}{}",
        String::from_utf8_lossy(&coverage.stdout),
        String::from_utf8_lossy(&coverage.stderr),
    );
}

#[test]
fn a_build_with_cached_and_new_shots_covers_the_landing() {
    let (volume, top) = cached_coverage_repo("cached-coverage", "");
    run_with_shots(&volume, &["mac.a."]);
    let first_shots = shots_runs(&volume, "mac.a.");
    run_with_shots(&volume, &["mac.a.", "mac.b."]);
    assert_eq!(shots_runs(&volume, "mac.a."), first_shots);
    assert_eq!(shots_runs(&volume, "mac.b.").len(), 1);
    assert_landing_coverage(&volume, &top);

    let log = volume.log();
    run_with_shots(&volume, &["mac.a.", "mac.b."]);
    assert_eq!(volume.log(), log, "the previous build already covers both");
}

#[test]
fn a_build_can_cover_shots_cached_by_separate_passing_builds() {
    let (volume, top) = cached_coverage_repo("split-shots", "");
    run_with_shots(&volume, &["mac.a."]);
    run_with_shots(&volume, &["mac.b."]);
    let a = shots_runs(&volume, "mac.a.");
    let b = shots_runs(&volume, "mac.b.");

    run_with_shots(&volume, &["mac.a.", "mac.b."]);
    assert_eq!(shots_runs(&volume, "mac.a."), a);
    assert_eq!(shots_runs(&volume, "mac.b."), b);
    assert_landing_coverage(&volume, &top);
}

#[test]
fn cached_shots_do_not_skip_a_changed_lane_without_coverage() {
    let (volume, top) = cached_coverage_repo("cached-other-lane", "");
    std::fs::create_dir(top.join("android")).unwrap();
    std::fs::write(top.join("android/Foo.txt"), "android change").unwrap();
    git_in(&top, &["add", "android/Foo.txt"]);
    git_in(&top, &["commit", "-q", "-m", "Change the Android app"]);
    git_in(&top, &["push", "-q", "origin", "work"]);
    volume.open_gate("droid");

    let built = volume.run(&[
        "app-build",
        "demo",
        "work",
        "one",
        "--shots",
        "mac",
        "mac.a.",
    ]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let mac_shots = shots_runs(&volume, "mac.a.");
    run_with_shots(&volume, &["mac.a."]);
    assert_eq!(shots_runs(&volume, "mac.a."), mac_shots);
    assert!(volume.log().contains(&"end droid".to_string()));
    assert_landing_coverage(&volume, &top);
}

#[test]
fn a_cached_only_request_does_not_rebuild_for_an_unrequested_failed_prefix() {
    let (volume, _) = cached_coverage_repo(
        "cached-unrequested",
        "if [ \"$2\" = mac.b. ]; then echo 'builds shots: 1 pair(s) fail the run: mac.b.one' >&2; exit 1; fi",
    );
    run_with_shots(&volume, &["mac.a."]);
    let failed = volume.run(&["app-build", "demo", "work", "--shots", "mac.b."]);
    assert!(!failed.status.success());
    let log = volume.log();
    run_with_shots(&volume, &["mac.a."]);
    assert_eq!(volume.log(), log);
}

#[test]
fn a_passing_recheck_reports_the_failed_source_and_runs_the_requested_test() {
    let volume = Volume::new("cache-failure-with-test");
    volume.app_repo_with(&format!(
        "{SHOTS_CONFIG}test mac ./two.sh {{class}}\nsources mac declared-mac\n"
    ));
    let top = volume.dir.join("checkout");
    commit_shots(
        &top,
        "trap 'echo \"builds shots: 1 pair(s) fail the run: mac.a.one\" >&2; exit 1' EXIT",
    );
    std::fs::write(top.join("declared-mac/Foo.swift"), "struct Foo {}\n").unwrap();
    git_in(&top, &["add", "declared-mac/Foo.swift"]);
    git_in(&top, &["commit", "-q", "-m", "Declare the test class"]);
    git_in(&top, &["push", "-q", "origin", "main"]);
    volume.open_gate("one");
    volume.open_gate("two");
    let first = volume.run(&["app-build", "demo", "main", "--shots", "mac.a."]);
    assert_eq!(first.status.code(), Some(1));
    let comparisons = shots_runs(&volume, "mac.a.");
    assert_eq!(comparisons.len(), 1);

    let commit = git_in(&top, &["rev-parse", "main"]);
    let capture = volume.dir.join(format!(
        "volume/shots/demo/runs/{commit}/mac/26B131-17C529/mac.a./{}",
        env!("CAPTURE_SOURCE")
    ));
    std::fs::rename(
        capture.join(env!("COMPARISON_SOURCE")),
        capture.join("old-comparison"),
    )
    .unwrap();

    let mixed = volume.run(&[
        "app-build",
        "demo",
        "main",
        "--shots",
        "mac.a.",
        "--test",
        "Foo",
    ]);
    assert_eq!(mixed.status.code(), Some(1));
    let said = String::from_utf8_lossy(&mixed.stderr);
    assert!(said.contains("which exited 0"), "{said}");
    assert!(
        said.contains("The source comparison for mac.a. failed with exit status 1."),
        "{said}"
    );
    assert_eq!(shots_runs(&volume, "mac.a."), comparisons);
    assert!(volume.log().iter().any(|line| line == "end two"));
}

/// A branch whose merge base has no cached run does not wait for another
/// build to make it and is not told to rebase: its lane builds the merge
/// base, runs the prefix there with `--base`, and then builds the branch,
/// all in the turn it waited for once.
#[test]
fn a_branch_whose_merge_base_has_no_run_builds_the_base_and_the_branch_in_one_lane_turn() {
    let volume = Volume::new("base-same-turn");
    volume.app_repo_with(SHOTS_CONFIG);
    let top = volume.dir.join("checkout");
    let base = commit_shots(&top, "");
    branch_from_main(&top, "work", "mac/Foo.txt");
    volume.open_gate("one");
    let built = volume.run(&["app-build", "demo", "work", "--shots", "mac.a."]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(built.status.success(), "{said}");
    let runs = shots_runs(&volume, "mac.a.");
    assert_eq!(runs.len(), 2, "{:?}\n{said}", volume.log());
    assert!(
        runs[0].contains("--base") && !runs[0].contains("--before"),
        "{runs:?}"
    );
    assert!(
        runs[1].contains("--before") && !runs[1].contains("--base"),
        "{runs:?}"
    );
    assert!(
        runs[1].contains(&format!("/runs/{base}/mac/26B131-17C529/")),
        "the branch compares with the run of its merge base made in the host's environment: {runs:?}"
    );
    assert_eq!(
        said.matches("waiting for demo's mac app lock").count(),
        1,
        "the base and the branch build in one turn of the lane's queue: {said}"
    );
    assert!(!said.contains("rebase"), "{said}");
    let log = volume.log();
    let at = |line: &str| log.iter().position(|l| l == line).unwrap();
    assert!(
        at("end one") < at(&runs[0]),
        "the merge base is built before its shots run: {log:?}"
    );
}

/// A branch cut before origin/main moved compares with the cached run of its
/// merge base, builds no run at the tip and is not told to rebase.
#[test]
fn a_branch_behind_origin_mains_tip_compares_with_its_merge_bases_run_and_is_not_told_to_rebase() {
    let volume = Volume::new("base-behind-tip");
    volume.app_repo_with(SHOTS_CONFIG);
    let top = volume.dir.join("checkout");
    let base = commit_shots(&top, "");
    branch_from_main(&top, "work", "mac/Foo.txt");
    std::fs::write(top.join("notes.txt"), "main moved\n").unwrap();
    git_in(&top, &["add", "notes.txt"]);
    git_in(&top, &["commit", "-q", "-m", "Move main"]);
    git_in(&top, &["push", "-q", "origin", "main"]);
    volume.open_gate("one");
    // Caches the run of the merge base.
    let first = volume.run(&["app-build", "demo", &base, "--shots", "mac.a."]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(shots_runs(&volume, "mac.a.").len(), 1);
    let built = volume.run(&["app-build", "demo", "work", "--shots", "mac.a."]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(built.status.success(), "{said}");
    let runs = shots_runs(&volume, "mac.a.");
    assert_eq!(
        runs.len(),
        2,
        "no run at the tip, none at the base: {runs:?}"
    );
    assert!(
        runs[1].contains("--before") && runs[1].contains(&format!("/runs/{base}/mac/")),
        "{runs:?}"
    );
    assert!(!runs[1].contains("--base"), "{runs:?}");
    assert!(!said.contains("rebase"), "{said}");
}

/// A shots prefix that starts no shot declared at the merge base compares
/// with nothing: every scene it selects is new, and no base run is built.
#[test]
fn a_prefix_that_starts_no_shot_declared_at_the_merge_base_compares_with_nothing_and_builds_no_base_run()
 {
    let volume = Volume::new("base-new-prefix");
    volume.app_repo_with(SHOTS_CONFIG);
    let top = volume.dir.join("checkout");
    commit_shots(&top, "");
    git_in(&top, &["checkout", "-q", "-b", "work"]);
    declare_scenes(
        &top,
        "declared-mac",
        "New.swift",
        "let scenes = [\"mac.new.one\"]\n",
    );
    std::fs::create_dir_all(top.join("mac")).unwrap();
    std::fs::write(top.join("mac/Foo.txt"), "work\n").unwrap();
    git_in(&top, &["add", "declared-mac", "mac"]);
    git_in(&top, &["commit", "-q", "-m", "Add a screen"]);
    git_in(&top, &["push", "-q", "origin", "work"]);
    git_in(&top, &["checkout", "-q", "main"]);
    volume.open_gate("one");
    let built = volume.run(&["app-build", "demo", "work", "--shots", "mac.new."]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(built.status.success(), "{said}");
    let runs = shots_runs(&volume, "mac.new.");
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(
        !runs[0].contains("--before") && !runs[0].contains("--base"),
        "{runs:?}"
    );
    assert!(
        said.contains("starts no shot declared at the merge base"),
        "{said}"
    );
}

/// A lane with two shots prefixes starts the shots program's capture once for
/// both, then runs each prefix without capturing; the merge base's runs do the
/// same.
#[test]
fn a_lane_with_two_shots_prefixes_captures_once_and_runs_each_prefix_without_capturing() {
    let volume = Volume::new("capture-once");
    volume.app_repo_with(SHOTS_CONFIG);
    let top = volume.dir.join("checkout");
    commit_shots(&top, "");
    declare_scenes(
        &top,
        "declared-mac",
        "Shots.swift",
        "let scenes = [\"mac.a.one\", \"mac.b.one\"]\n",
    );
    git_in(&top, &["add", "declared-mac"]);
    git_in(&top, &["commit", "-q", "-m", "Declare a second screen"]);
    git_in(&top, &["push", "-q", "origin", "main"]);
    branch_from_main(&top, "work", "mac/Foo.txt");
    volume.open_gate("one");
    let built = volume.run(&[
        "app-build",
        "demo",
        "work",
        "--shots",
        "mac.a.",
        "--shots",
        "mac.b.",
    ]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(built.status.success(), "{said}");
    let log = volume.log();
    let captures: Vec<&String> = log
        .iter()
        .filter(|line| line.contains("--capture-only"))
        .collect();
    assert_eq!(
        captures.len(),
        2,
        "one at the merge base, one at the branch: {log:?}"
    );
    for capture in captures {
        assert!(
            capture.starts_with("shots --select mac.a. --select mac.b. ")
                && !capture.contains("--base")
                && !capture.contains("--before"),
            "{capture}"
        );
    }
    for prefix in ["mac.a.", "mac.b."] {
        let runs: Vec<String> = shots_runs(&volume, prefix)
            .into_iter()
            .filter(|run| !run.contains("--capture-only"))
            .collect();
        assert_eq!(runs.len(), 2, "{log:?}");
        assert!(
            runs.iter().all(|run| run.contains("--no-capture")),
            "{runs:?}"
        );
    }
}

/// A base run whose capture fails, with no findings, fails the branch's
/// build, which prints what the capture said.
#[test]
fn a_base_run_whose_capture_fails_fails_the_branch_build_with_its_output() {
    let volume = Volume::new("base-capture-fails");
    volume.app_repo_with(SHOTS_CONFIG);
    let project = volume.run(&["lists", "init", "demo", "--repos", "demo", "--by", "test"]);
    assert!(
        project.status.success(),
        "{}",
        String::from_utf8_lossy(&project.stderr)
    );
    let top = volume.dir.join("checkout");
    commit_shots(
        &top,
        "case \" $* \" in *\" --base \"*) echo 'base-capture-crashed' >&2; exit 1;; esac",
    );
    branch_from_main(&top, "work", "mac/Foo.txt");
    volume.open_gate("one");
    let abbreviated = git_in(&top, &["rev-parse", "--short", "work"]);
    let built = volume.run(&["app-build", "demo", &abbreviated, "--shots", "mac.a."]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(!built.status.success(), "{said}");
    assert!(said.contains("base-capture-crashed"), "{said}");
    assert!(said.contains("gave nothing to compare with"), "{said}");
    let runs = shots_runs(&volume, "mac.a.");
    assert_eq!(
        runs.len(),
        1,
        "the branch's own run never started: {runs:?}"
    );
    let passing = volume.run(&["app-build", "demo", "work", "one"]);
    assert!(
        passing.status.success(),
        "{}",
        String::from_utf8_lossy(&passing.stderr)
    );

    git_in(&volume.dir, &["clone", "-q", "origin.git", "demo"]);
    let worktree = volume.dir.join("demo");
    let target = git_in(&worktree, &["rev-parse", "origin/work"]);
    let coverage = volume.run(&[
        "app-build",
        "--landing-coverage",
        worktree.to_str().unwrap(),
        &target,
        "work",
    ]);
    let said = String::from_utf8_lossy(&coverage.stdout);
    assert_eq!(coverage.status.code(), Some(1), "{said}");
    assert!(
        said.contains("did not cover the shots prefixes mac.a."),
        "{said}"
    );
}

/// Two builds with one merge base, in two checkouts of the lane at once: one
/// builds the base run while the other waits for it, then reads it from the
/// cache and builds none.
#[test]
fn two_builds_with_one_merge_base_build_its_run_once_and_the_other_waits_for_it() {
    let volume = Volume::new("base-lock");
    volume.app_repo_with(SHOTS_CONFIG);
    let top = volume.dir.join("checkout");
    commit_shots(&top, "");
    branch_from_main(&top, "work-a", "mac/A.txt");
    branch_from_main(&top, "work-b", "mac/B.txt");
    let second = volume.dir.join("checkout-mac-2");
    git_in(
        &top,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            second.to_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(volume.dir.join("volume/demo/mac-2")).unwrap();
    std::fs::write(
        volume.dir.join("volume/demo/mac-2/.checkout"),
        second.to_str().unwrap(),
    )
    .unwrap();
    std::fs::write(
        volume.dir.join("volume/demo/app-mac-checkouts"),
        "mac-1\nmac-2\n",
    )
    .unwrap();
    let start = |branch: &str, errors: &Path| {
        Spawned::start(
            volume
                .builds()
                .args(["app-build", "demo", branch, "--shots", "mac.a."])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(errors).unwrap()),
        )
    };
    let (a_said, b_said) = (volume.dir.join("a.err"), volume.dir.join("b.err"));
    let mut a = start("work-a", &a_said);
    // A holds the base run's lock, in its build of the merge base.
    volume.until_logged("start one");
    let mut b = start("work-b", &b_said);
    until("B waits for A's run of the merge base", || {
        std::fs::read_to_string(&b_said)
            .unwrap_or_default()
            .contains("being made by another build")
    });
    volume.open_gate("one");
    assert!(a.wait().unwrap().success());
    assert!(b.wait().unwrap().success());
    let runs = shots_runs(&volume, "mac.a.");
    assert_eq!(runs.len(), 3, "{runs:?}");
    assert_eq!(
        runs.iter().filter(|run| run.contains("--base")).count(),
        1,
        "one build made the base run: {runs:?}"
    );
}

/// A run of the merge base that another environment made is no run to compare
/// with: a branch built in a host whose macOS differs builds the merge base
/// again, in its own environment, and compares with that run.
#[test]
fn a_run_of_the_merge_base_made_in_another_environment_is_no_run_to_compare_with() {
    let volume = Volume::new("base-environment");
    volume.app_repo_with(SHOTS_CONFIG);
    let top = volume.dir.join("checkout");
    let base = commit_shots(&top, "");
    branch_from_main(&top, "work", "mac/Foo.txt");
    volume.open_gate("one");
    let first = volume.run(&["app-build", "demo", &base, "--shots", "mac.a."]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(shots_runs(&volume, "mac.a.").len(), 1);
    // The host's macOS changes, as when the person upgrades it.
    std::fs::write(volume.dir.join("macos-build"), "27A100\n").unwrap();
    let built = volume.run(&["app-build", "demo", "work", "--shots", "mac.a."]);
    let said = String::from_utf8_lossy(&built.stderr).into_owned();
    assert!(built.status.success(), "{said}");
    let runs = shots_runs(&volume, "mac.a.");
    assert_eq!(runs.len(), 3, "{runs:?}");
    assert!(runs[1].contains("--base"), "{runs:?}");
    assert!(
        runs[2].contains(&format!("/runs/{base}/mac/27A100-17C529/")),
        "{runs:?}"
    );
    assert!(
        said.contains("environment 27A100-17C529"),
        "the lane says which environment it draws in: {said}"
    );
}
