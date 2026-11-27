//! A lane's app checkout on a host, end to end: real `builds app-build`
//! processes on a scratch build volume, with a fake ssh (a program that runs
//! the command on this Mac with a second folder as the host's home) and a
//! scratch repo `demo` whose clone sits in that home. Never a real host.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod programs;

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

/// The hang step's limit counts from before the host's builds starts, so it
/// covers that start on a loaded Mac too: the step has to be running when the
/// limit passes.
const APP_BUILD: &str = "step mac xcode ./xcode.sh\nstep mac quick ./hello.sh\nstep android hello ./hello.sh\nstep android hang ./hang.sh\nstep android signed ./hello.sh\nhere signed\nlimit hang 10s\ntest android ./t.sh {class}\ntest mac ./t.sh {class}\nopens-windows mac\nresults android out\nmac build ./t.sh Sleepy\nmac app out.app\nmac bundle demo.test\n";

struct World {
    dir: PathBuf,
    source: PathBuf,
}

fn run(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl World {
    fn new(name: &str) -> World {
        // Short: the dashboard's socket path may not pass 104 bytes.
        let dir = PathBuf::from(format!("/private/tmp/br-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = dir.join("src/demo");
        let origin = dir.join("origin.git");
        let remote_home = dir.join("remote-home");
        let second_home = dir.join("remote-home-2");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(remote_home.join("dev")).unwrap();
        std::fs::create_dir_all(second_home.join("dev")).unwrap();
        std::fs::create_dir_all(remote_home.join("Library/Android/sdk")).unwrap();
        // Each host's `builds`, whose setup an app checkout's setup runs. A
        // real one would link the build folders into this Mac's build volume.
        for home in [&remote_home, &second_home] {
            std::fs::create_dir_all(home.join(".local/bin")).unwrap();
            script(&home.join(".local/bin/builds"), "#!/bin/sh\n");
        }
        std::fs::create_dir_all(dir.join("home")).unwrap();
        programs::programs_folder(&dir.join("home"));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("volume/demo")).unwrap();
        run(&source, &["init", "-q", "-b", "main"]);
        run(&source, &["config", "user.email", "test@example.com"]);
        run(&source, &["config", "user.name", "test"]);
        std::fs::write(source.join(".build-dirs"), "target 1G\n").unwrap();
        std::fs::write(source.join(".gitignore"), "target\nout\n").unwrap();
        std::fs::write(source.join(".app-build"), APP_BUILD).unwrap();
        script(
            &source.join("hello.sh"),
            "#!/bin/sh\necho \"hello from $(basename \"$PWD\")\"\necho oops >&2\n",
        );
        script(
            &source.join("hang.sh"),
            "#!/bin/sh\necho $$ > \"$HOME/hang.pid\"\nexec sleep 300\n",
        );
        script(
            &source.join("xcode.sh"),
            "#!/bin/sh\nif [ -e \"$HOME/xcode-broken\" ]; then echo \"A required plugin failed to load. Please ensure system content is up-to-date, try running 'xcodebuild -runFirstLaunch'.\" >&2; exit 70; fi\necho \"built in $(basename \"$HOME\")\"\n",
        );
        script(
            &source.join("t.sh"),
            "#!/bin/sh\nif [ \"$1\" = Sleepy ]; then echo $$ > \"$HOME/sleepy.pid\"; exec sleep 300; fi\nmkdir -p out\necho \"report of $1\" > out/report\necho \"ran $1\"\n[ \"$1\" != Bad ] || exit 4\n",
        );
        run(&source, &["add", "-A"]);
        run(&source, &["commit", "-q", "-m", "Start"]);
        run(
            &dir,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        run(
            &source,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        run(&source, &["push", "-q", "origin", "main"]);
        run(
            &dir,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                remote_home.join("dev/demo").to_str().unwrap(),
            ],
        );
        run(
            &dir,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                second_home.join("dev/demo").to_str().unwrap(),
            ],
        );
        // The hosts run on this Mac, so each reads its screen lock from this
        // ioreg: whether this Mac's screen is locked never holds a window
        // step. BUILDS_VOLUME names no folder, so each host's builds root is
        // in its own home, not on this Mac's build volume.
        script(
            &dir.join("bin/ioreg"),
            "#!/bin/sh\necho '\"IOConsoleLocked\" = No'\n",
        );
        script(
            &dir.join("bin/ssh"),
            &format!(
                r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-i|-l) shift 2;;
    -*) shift;;
    *) break;;
  esac
done
case "$1" in
  user@m2) home='{second}';;
  *) home='{home}';;
esac
shift
cd "$home" && HOME="$home" BUILDS_VOLUME="$home/no-volume" PATH='{bin}':"$PATH" exec sh -c "$*"
"#,
                bin = dir.join("bin").display(),
                home = remote_home.display(),
                second = second_home.display()
            ),
        );
        std::fs::write(dir.join("hosts"), "m1 user@m1 10\n").unwrap();
        std::fs::write(dir.join("volume/demo/app-android-checkouts"), "m1:app-1\n").unwrap();
        World { dir, source }
    }

    /// A `builds` command with <args>, run in the repo with the world's
    /// volume, home, store and ssh.
    fn builds(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BUILDS);
        command
            .args(args)
            .current_dir(&self.source)
            .env("BUILDS_ROOT", self.dir.join("volume"))
            .env("HOME", self.dir.join("home"))
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("BUILDS_AGENT")
            .env("BUILDS_LISTS", self.dir.join("lists"))
            .env("BUILDS_SSH", self.dir.join("bin/ssh"));
        command
    }

    fn app_build(&self, args: &[&str]) -> Output {
        self.builds(&[&["app-build", "demo"], args].concat())
            .output()
            .unwrap()
    }

    /// Starts `builds <args>`, reading its stderr line by line.
    fn start(&self, args: &[&str]) -> Running {
        let mut child = self
            .builds(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let stderr = child.stderr.take().unwrap();
        let said = Arc::new(Mutex::new(String::new()));
        let (line_sent, lines) = channel();
        let heard = said.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                heard.lock().unwrap().push_str(&format!("{line}\n"));
                if line_sent.send(line).is_err() {
                    break;
                }
            }
        });
        let (status_sent, ended) = channel();
        std::thread::spawn(move || {
            let _ = status_sent.send(child.wait().unwrap());
        });
        Running {
            pid,
            lines,
            said,
            ended,
            status: None,
        }
    }

    /// Makes the project store that records demo's app builds, and puts the
    /// mac lane's app checkout m1:mac-1 on host m1.
    fn with_store_and_mac_checkout(&self) {
        let made = self
            .builds(&["lists", "init", "demo", "--repos", "demo", "--by", "person"])
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        std::fs::write(self.dir.join("volume/demo/app-mac-checkouts"), "m1:mac-1\n").unwrap();
    }

    /// The outcome of each recorded app build run, and the result of each of
    /// its steps, oldest first.
    fn recorded_runs(&self) -> Vec<(String, Vec<(String, String)>)> {
        let conn = rusqlite::Connection::open(self.dir.join("lists/demo/builds.sqlite")).unwrap();
        let runs: Vec<(i64, String)> = conn
            .prepare("SELECT id, outcome FROM app_build_runs ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        runs.into_iter()
            .map(|(id, outcome)| {
                let steps = conn
                    .prepare(
                        "SELECT name, result FROM app_build_steps WHERE run_id = ?1 ORDER BY position",
                    )
                    .unwrap()
                    .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                (outcome, steps)
            })
            .collect()
    }
}

/// How long a test waits for what a build prints or does before it fails.
const PATIENCE: Duration = Duration::from_secs(60);

/// A builds command the test started, stopped when the test ends however it
/// ends.
struct Running {
    pid: i32,
    /// Each line of its stderr, as it prints it.
    lines: Receiver<String>,
    /// All of its stderr so far.
    said: Arc<Mutex<String>>,
    /// Its exit status, once it has ended.
    ended: Receiver<ExitStatus>,
    status: Option<ExitStatus>,
}

impl Running {
    fn signal(&self, signal: i32) {
        assert_eq!(unsafe { libc::kill(self.pid, signal) }, 0);
    }

    /// What it has printed on stderr so far.
    fn said(&self) -> String {
        self.said.lock().unwrap().clone()
    }

    /// Waits until it prints a line on stderr that holds <text>.
    fn until_said(&self, text: &str) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains(text) => return,
                Ok(_) => {}
                Err(_) => panic!("never saw {text:?} in:\n{}", self.said()),
            }
        }
    }

    /// Its exit status once it has ended, or None when it still runs after
    /// <patience>.
    fn ended_within(&mut self, patience: Duration) -> Option<ExitStatus> {
        self.status = self.ended.recv_timeout(patience).ok();
        self.status
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.status.is_none() {
            // SIGTERM: builds passes it to the step it runs, which SIGKILL
            // would leave running.
            unsafe { libc::kill(self.pid, libc::SIGTERM) };
            let _ = self.ended.recv();
        }
    }
}

/// What a step writes to the FIFO <path>, read as soon as the step writes
/// it; the test fails when no step writes it within `PATIENCE`.
fn read_fifo(path: &Path) -> String {
    let (sent, read) = channel();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let _ = sent.send(std::fs::read_to_string(&path).unwrap());
    });
    read.recv_timeout(PATIENCE)
        .unwrap_or_else(|_| panic!("no step wrote to the FIFO"))
}

/// Opens the lock file <path>, taking its lock, or None while a process
/// holds it. The lock lasts as long as the file stays open.
fn try_lock(path: &Path) -> Option<std::fs::File> {
    use std::os::unix::io::AsRawFd;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .unwrap();
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    taken.then_some(file)
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_step_runs_in_the_hosts_checkout_and_its_output_comes_back_behind_the_checkouts_name() {
    let world = World::new("step");
    let output = world.app_build(&["main", "hello"]);
    let (out, err) = (text(&output.stdout), text(&output.stderr));
    assert!(output.status.success(), "{out}\n{err}");
    assert!(out.contains("[android m1:app-1] hello from app-1"), "{out}");
    assert!(err.contains("[android m1:app-1] oops"), "{err}");
    let worktree = world.dir.join("remote-home/dev/demo-worktrees/app-1");
    assert!(worktree.join("hello.sh").exists());
    assert!(
        world
            .dir
            .join("volume/demo/m1:app-1/.last-app-build")
            .exists(),
        "the build is recorded on this Mac under the entry's name"
    );
}

#[test]
fn a_run_of_tests_copies_the_results_back_whether_it_passes_or_fails() {
    let world = World::new("pick");
    let output = world.app_build(&["main", "hello", "--test", "android", "Good"]);
    let (out, err) = (text(&output.stdout), text(&output.stderr));
    assert!(output.status.success(), "{out}\n{err}");
    let report = world.dir.join("volume/demo/m1:app-1/out/report");
    assert_eq!(
        std::fs::read_to_string(&report).unwrap(),
        "report of Good\n"
    );
    let output = world.app_build(&[
        "--lane",
        "android",
        "main",
        "--built-ref",
        "main",
        "--test",
        "android",
        "Bad",
    ]);
    let (out, err) = (text(&output.stdout), text(&output.stderr));
    assert!(!output.status.success(), "{out}\n{err}");
    assert!(err.contains("exit 4"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&report).unwrap(),
        "report of Bad\n",
        "the failing run's report came back too"
    );
}

#[test]
fn a_step_past_its_limit_is_stopped_on_the_host() {
    let world = World::new("limit");
    let output = world.app_build(&["main", "hang"]);
    let err = text(&output.stderr);
    assert!(!output.status.success(), "{err}");
    assert!(err.contains("ran past its limit of 10s"), "{err}");
    let pid = std::fs::read_to_string(world.dir.join("remote-home/hang.pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    assert!(!alive(pid), "the step's process on the host is gone");
}

/// Whether process <pid> exists.
fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn a_step_the_repo_keeps_on_this_mac_never_takes_a_hosts_checkout() {
    let world = World::new("here");
    let output = world.app_build(&["main", "signed"]);
    let err = text(&output.stderr);
    assert!(!output.status.success(), "{err}");
    assert!(err.contains("on this Mac"), "{err}");
    assert!(
        !world
            .dir
            .join("remote-home/dev/demo-worktrees/app-1")
            .exists(),
        "nothing was sent to the host"
    );
}

/// A mac lane step that fails because the host's Xcode cannot start runs
/// again in a checkout on another host. builds records the host, so the next
/// build skips it from the start.
#[test]
fn a_build_skips_and_records_a_host_whose_xcode_cannot_start() {
    let world = World::new("xcode");
    std::fs::write(world.dir.join("hosts"), "m1 user@m1 10\nm2 user@m2 10\n").unwrap();
    std::fs::write(
        world.dir.join("volume/demo/app-mac-checkouts"),
        "m1:mac-1\nm2:mac-1\n",
    )
    .unwrap();
    std::fs::write(world.dir.join("remote-home/xcode-broken"), "").unwrap();
    let output = world.app_build(&["main", "xcode"]);
    let (out, err) = (text(&output.stdout), text(&output.stderr));
    assert!(output.status.success(), "{out}\n{err}");
    assert!(
        err.contains("[mac m1:mac-1] A required plugin failed"),
        "{err}"
    );
    assert!(err.contains("on host m1, Xcode cannot start"), "{err}");
    assert!(
        out.contains("[mac m2:mac-1] built in remote-home-2"),
        "{out}"
    );
    let record = world.dir.join("host-xcode-failures/m1");
    assert!(
        std::fs::read_to_string(&record)
            .unwrap()
            .contains("xcodebuild -runFirstLaunch"),
        "the record keeps what xcodebuild said"
    );
    let output = world.app_build(&["main", "xcode"]);
    let (out, err) = (text(&output.stdout), text(&output.stderr));
    assert!(output.status.success(), "{out}\n{err}");
    assert!(
        !out.contains("m1:mac-1") && !err.contains("m1:mac-1"),
        "{out}\n{err}"
    );
}

/// A stop signal reaches an app build while its lane waits between steps for
/// its host's display, which another run holds. The build ends, its lane's
/// lock is free, and builds records the run as stopped.
#[test]
fn a_stop_signal_ends_an_app_build_that_waits_for_its_hosts_display() {
    let world = World::new("stop-wait");
    world.with_store_and_mac_checkout();
    let display = try_lock(&world.dir.join("volume/.app-windows/m1")).unwrap();
    let mut build = world.start(&[
        "app-build",
        "demo",
        "main",
        "quick",
        "--test",
        "mac",
        "Good",
    ]);
    build.until_said("to release m1's display");
    build.signal(libc::SIGTERM);
    let status = build
        .ended_within(Duration::from_secs(20))
        .unwrap_or_else(|| panic!("the build ignored SIGTERM: {}", build.said()));
    assert!(!status.success(), "{}", build.said());
    assert!(
        try_lock(&world.dir.join("volume/demo/app-mac.locks/m1:mac-1")).is_some(),
        "the lane's lock is free"
    );
    drop(display);
    assert_eq!(
        world.recorded_runs(),
        vec![(
            "stopped".to_string(),
            vec![("quick".to_string(), "passed".to_string())]
        )]
    );
}

/// A stop signal reaches an app build while a step that opens windows runs
/// on the host. The step's process on the host ends, the lane's lock and the
/// host's display lock are free, and builds records the run and the step as
/// stopped.
#[test]
fn a_stop_signal_ends_an_app_builds_step_on_the_host_and_frees_its_locks() {
    let world = World::new("stop-step");
    world.with_store_and_mac_checkout();
    let pid_fifo = world.dir.join("remote-home/sleepy.pid");
    make_fifo(&pid_fifo);
    let mut build = world.start(&[
        "app-build",
        "demo",
        "main",
        "quick",
        "--test",
        "mac",
        "Sleepy",
    ]);
    let pid = read_fifo(&pid_fifo).trim().parse::<i32>().unwrap();
    build.signal(libc::SIGTERM);
    let status = build
        .ended_within(Duration::from_secs(20))
        .unwrap_or_else(|| panic!("the build ignored SIGTERM: {}", build.said()));
    assert!(!status.success(), "{}", build.said());
    assert!(!alive(pid), "the step's process on the host is gone");
    assert!(
        try_lock(&world.dir.join("volume/demo/app-mac.locks/m1:mac-1")).is_some(),
        "the lane's lock is free"
    );
    assert!(
        try_lock(&world.dir.join("volume/.app-windows/m1")).is_some(),
        "the host's display lock is free"
    );
    assert_eq!(
        world.recorded_runs(),
        vec![(
            "stopped".to_string(),
            vec![
                ("quick".to_string(), "passed".to_string()),
                ("test Sleepy".to_string(), "stopped".to_string())
            ]
        )]
    );
}

/// Makes the FIFO <path>, which a step writes its pid to.
fn make_fifo(path: &Path) {
    let path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
}

/// A stop signal reaches a build-app while its build runs on the host. The
/// build's process on the host ends, the lane's lock is free, and build-app
/// says the signal stopped it.
#[test]
fn a_stop_signal_ends_a_build_apps_build_on_the_host_and_frees_its_lock() {
    let world = World::new("stop-build-app");
    world.with_store_and_mac_checkout();
    let pid_fifo = world.dir.join("remote-home/sleepy.pid");
    make_fifo(&pid_fifo);
    let mut build = world.start(&["build-app", "demo", "mac", "main"]);
    let pid = read_fifo(&pid_fifo).trim().parse::<i32>().unwrap();
    build.signal(libc::SIGTERM);
    let status = build
        .ended_within(Duration::from_secs(20))
        .unwrap_or_else(|| panic!("build-app ignored SIGTERM: {}", build.said()));
    // The exit status a shell gives a program that SIGTERM ended: the daemon
    // shows such a run as stopped.
    assert_eq!(status.code(), Some(128 + libc::SIGTERM), "{}", build.said());
    assert!(
        build.said().contains("stopped on SIGTERM"),
        "{}",
        build.said()
    );
    assert!(!alive(pid), "the build's process on the host is gone");
    assert!(
        try_lock(&world.dir.join("volume/demo/app-mac.locks/m1:mac-1")).is_some(),
        "the lane's lock is free"
    );
}

/// A stop signal reaches a build-app that waits in the lane's queue for the
/// lock another build holds. build-app ends and says the signal stopped it.
#[test]
fn a_stop_signal_ends_a_build_app_that_waits_for_its_turn() {
    let world = World::new("stop-build-app-wait");
    world.with_store_and_mac_checkout();
    let lane = try_lock(&world.dir.join("volume/demo/app-mac.locks/m1:mac-1")).unwrap();
    let mut build = world.start(&["build-app", "demo", "mac", "main"]);
    build.until_said("waiting for demo's mac app lock");
    build.signal(libc::SIGTERM);
    let status = build
        .ended_within(Duration::from_secs(20))
        .unwrap_or_else(|| panic!("build-app ignored SIGTERM: {}", build.said()));
    // The exit status a shell gives a program that SIGTERM ended: the daemon
    // shows such a run as stopped.
    assert_eq!(status.code(), Some(128 + libc::SIGTERM), "{}", build.said());
    assert!(
        build.said().contains("stopped on SIGTERM"),
        "{}",
        build.said()
    );
    drop(lane);
}
