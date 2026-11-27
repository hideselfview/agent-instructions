//! `builds lock` and `builds health` on a scratch repo and build volume, with
//! `sleep` standing in for a build: a waiting command does not start while any
//! process of the holder's build lives, whether the holder is killed outright
//! or asked to stop; health counts a build under the lock as the build of
//! the worktree it is for, and finds the holder by the lock file's open
//! descriptor once `builds lock` itself is gone; and health counts a `builds
//! cargo` run whose cargo runs on another host as a build of the worktree it
//! starts in.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

mod fake_launchctl;
use fake_launchctl::FakeLaunchctl;
mod spawned;
use spawned::{Spawned, wait_for_free_lanes};

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");
const PATIENCE: Duration = Duration::from_secs(30);

/// A repo `demo` with origin/main, its app checkout mac-1, and side-2,
/// claimed three hours ago, where agent B made one commit three hours ago and
/// none since. The demo has a build volume of its own.
struct Demo {
    dir: PathBuf,
    side: PathBuf,
    app: PathBuf,
    work: String,
    /// The launchctl that the demo's builds commands find.
    launchctl: FakeLaunchctl,
    started: Vec<Spawned>,
}

fn git(top: &Path, args: &[&str], date: Option<&str>) -> String {
    let mut command = Command::new("git");
    command.arg("-C").arg(top).args(args);
    if let Some(date) = date {
        command
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

impl Demo {
    fn new(name: &str) -> Demo {
        let dir = PathBuf::from(format!("/private/tmp/bl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("demo");
        let app = dir.join("demo-worktrees/mac-1");
        let side = dir.join("demo-worktrees/side-2");
        let three_hours_ago = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 3 * 3600;
        let old = format!("{three_hours_ago} +0000");
        // Health counts every `builds app-build <repo> <branch>` process on
        // the machine as a build of the worktree on that branch, so this
        // branch is named for this run: another test's or agent's
        // `app-build demo work` must not make side-2 look busy.
        let branch = format!("bl-{name}-{}", std::process::id());
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"], None);
        git(&main, &["config", "user.email", "test@example.com"], None);
        git(&main, &["config", "user.name", "test"], None);
        std::fs::write(main.join(".build-dirs"), "target 1G\n").unwrap();
        std::fs::write(main.join(".gitignore"), "target\n").unwrap();
        git(&main, &["add", ".build-dirs", ".gitignore"], None);
        git(&main, &["commit", "-q", "-m", "Start"], Some(&old));
        let origin = dir.join("origin.git");
        let made = Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&origin)
            .output()
            .unwrap();
        assert!(made.status.success());
        git(
            &main,
            &["remote", "add", "origin", origin.to_str().unwrap()],
            None,
        );
        git(&main, &["push", "-q", "origin", "main"], None);
        git(
            &main,
            &["worktree", "add", "-q", "--detach", app.to_str().unwrap()],
            None,
        );
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                side.to_str().unwrap(),
                "origin/main",
            ],
            None,
        );
        std::fs::write(side.join("file"), "change\n").unwrap();
        git(&side, &["add", "file"], None);
        git(&side, &["commit", "-q", "-m", "Work"], Some(&old));
        let work = git(&side, &["rev-parse", "HEAD"], None);
        for (name, top) in [("main", &main), ("mac-1", &app), ("side-2", &side)] {
            let root = dir.join("volume/demo").join(name);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join(".checkout"), format!("{}\n", top.display())).unwrap();
        }
        let launchctl = FakeLaunchctl::new(&dir.join("bin"));
        let demo = Demo {
            launchctl,
            dir,
            side,
            app,
            work,
            started: Vec::new(),
        };
        let app = demo.app.to_str().unwrap().to_string();
        let side = demo.side.to_str().unwrap().to_string();
        assert!(demo.run(&["app-checkout", "mac", &app]).status.success());
        assert!(
            demo.run(&["claim", &side, "agent B"]).status.success(),
            "claim"
        );
        demo.date_claim(three_hours_ago);
        demo
    }

    /// Moves side-2's claim's start to <epoch>, as if `builds claim` had run
    /// then: the claim file is `<who>\t<YYYY-MM-DD HH:MM>`, in local time, and
    /// its last write, which health counts as the claim's last change, is
    /// <epoch> too.
    fn date_claim(&self, epoch: u64) {
        let file = PathBuf::from(git(&self.side, &["rev-parse", "--absolute-git-dir"], None))
            .join("builds-claim");
        let date = Command::new("date")
            .args(["-r", &epoch.to_string(), "+%Y-%m-%d %H:%M"])
            .output()
            .unwrap();
        assert!(date.status.success());
        let date = String::from_utf8(date.stdout).unwrap();
        let claim = std::fs::read_to_string(&file).unwrap();
        let (who, _) = claim.split_once('\t').unwrap();
        std::fs::write(&file, format!("{who}\t{}\n", date.trim())).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(epoch))
            .unwrap();
    }

    /// Returns a `builds` command that uses the demo's volume, lists and
    /// hosts, the fake ssh that `host_m1` writes, so it never reaches a real
    /// host, and the fake launchctl, which the commands that ssh runs inherit.
    fn builds(&self) -> Command {
        let mut command = Command::new(BUILDS);
        command
            .env("BUILDS_ROOT", self.dir.join("volume"))
            .env("BUILDS_POWER_RECORD", self.dir.join("power"))
            .env("BUILDS_TEMP", self.dir.join("temp"))
            .env("HOME", self.dir.join("home"))
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("BUILDS_AGENT")
            .env("BUILDS_LISTS", self.dir.join("lists"))
            .env("BUILDS_SSH", self.dir.join("bin/ssh"))
            .env("PATH", self.launchctl.path());
        command
    }

    /// Records host m1, whose home is <dir>/m1 with a clone of demo and
    /// <builds> as its `builds`, behind a fake ssh that runs each command on
    /// this Mac in that home. The ssh stays the command's parent, as a real
    /// one does, so its arguments still name the host. BUILDS_VOLUME names no
    /// folder, so the host's builds root is in its own home, not on this Mac's
    /// build volume.
    fn host_m1(&self, builds: &str) {
        let home = self.dir.join("m1");
        let origin = self.dir.join("origin.git");
        std::fs::create_dir_all(home.join("dev")).unwrap();
        git(
            &self.dir,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                home.join("dev/demo").to_str().unwrap(),
            ],
            None,
        );
        script(&home.join(".local/bin/builds"), builds);
        // A host takes a `cargo test` only with the node builds installs.
        script(&home.join(".cargo/bin/node"), "#!/bin/sh\necho v22.17.0\n");
        script(
            &self.dir.join("bin/ssh"),
            &format!(
                r#"#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2;;
    *) break;;
  esac
done
shift
cd '{home}' && HOME='{home}' BUILDS_VOLUME='{home}/no-volume' sh -c "$*"
"#,
                home = home.display()
            ),
        );
        // With a core speed recorded: health measures a host that has none by
        // setting it up (health_fix.rs), which replaces the stand-in builds
        // with the real one.
        std::fs::write(self.dir.join("hosts"), "m1 user@m1 10 1.00\n").unwrap();
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.builds().args(args).output().unwrap()
    }

    /// Starts `builds lock demo --lane mac <script>` in <cwd>, <script> run
    /// by sh with the arguments <args> after its name.
    fn lock(&mut self, cwd: &Path, script: &str, args: &[String]) -> i32 {
        let child = Spawned::start(
            self.builds()
                .current_dir(cwd)
                .args(["lock", "demo", "--lane", "mac", "sh", "-c", script, "sh"])
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        );
        let pid = child.id() as i32;
        self.started.push(child);
        pid
    }

    /// Starts a `builds lock demo --lane mac` that records in <out> whether
    /// <flag> existed when it started: `during` or `after`.
    fn waiter(&mut self, flag: &Path, out: &Path) -> Spawned {
        Spawned::start(
            self.builds()
                .args([
                    "lock",
                    "demo",
                    "--lane",
                    "mac",
                    "sh",
                    "-c",
                    r#"if [ -f "$1" ]; then echo after; else echo during; fi > "$2""#,
                    "sh",
                ])
                .arg(flag)
                .arg(out)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
    }

    /// `builds health --json`, which exits 1 when it finds something to act
    /// on: its JSON.
    fn health(&self) -> serde_json::Value {
        let output = self.run(&["health", "--json"]);
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "health said {}: {error}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    fn side_2(&self, health: &serde_json::Value) -> serde_json::Value {
        health["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|worktree| worktree["checkout"] == "demo/side-2")
            .unwrap_or_else(|| panic!("no side-2 in {health}"))
            .clone()
    }

    fn idle(&self, health: &serde_json::Value) -> bool {
        health["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["text"]
                    .as_str()
                    .unwrap()
                    .starts_with("worktree demo/side-2 ")
            })
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        self.started.clear();
        wait_for_free_lanes(&self.dir.join("volume/demo"));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Writes the program <text> at <path>.
fn script(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "never saw {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn path(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

fn kill(pid: i32, signal: i32) {
    unsafe { libc::kill(pid, signal) };
}

fn wait_for(child: &mut Child) -> std::process::ExitStatus {
    child.wait().unwrap()
}

/// The pid of the one child of <pid>.
fn child_of(pid: i32) -> i32 {
    let output = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_else(|| panic!("{pid} has no child"))
        .parse()
        .unwrap()
}

#[test]
fn a_repo_without_an_app_checkout_has_no_app_lock_to_take() {
    let demo = Demo::new("noapp");
    let output = demo.run(&["lock", "other", "--lane", "mac", "true"]);
    assert!(!output.status.success());
}

/// The holder killed outright (SIGKILL: nothing is passed on). Its build keeps
/// running and keeps the lock until it ends.
#[test]
fn a_killed_holders_build_keeps_the_lock_until_it_ends() {
    let mut demo = Demo::new("kill");
    let dir = demo.dir.clone();
    let holder = demo.lock(
        &dir,
        r#"touch "$1/started"; sleep 3; touch "$1/build.done""#,
        &[path(&dir)],
    );
    until("the build to start", || dir.join("started").exists());
    let mut waiter = demo.waiter(&dir.join("build.done"), &dir.join("waiter"));
    kill(holder, libc::SIGKILL);
    let index = demo
        .started
        .iter()
        .position(|child| child.id() as i32 == holder)
        .unwrap();
    let _ = wait_for(&mut demo.started[index]);
    assert!(wait_for(&mut waiter).success());
    assert_eq!(
        std::fs::read_to_string(dir.join("waiter")).unwrap(),
        "after\n",
        "the waiter started while the killed holder's build still ran"
    );
}

/// The holder asked to stop (SIGTERM). It passes the signal on to its build's
/// whole process group, the background sleep included, and the waiter starts
/// once all of it has exited, well before the sleep would have ended. Health
/// names the `builds lock` with the file open as the holder.
#[test]
fn a_stopped_holder_stops_its_whole_build_before_the_waiter_starts() {
    let mut demo = Demo::new("term");
    let dir = demo.dir.clone();
    let holder = demo.lock(
        &dir,
        r#"trap 'touch "$1/build.stopped"; exit 1' TERM; sleep 60 & touch "$1/started"; wait"#,
        &[path(&dir)],
    );
    until("the build to start", || dir.join("started").exists());
    let mut waiter = demo.waiter(&dir.join("build.stopped"), &dir.join("waiter"));
    let health = demo.health();
    assert_eq!(
        health["locks"][0]["holder"]["pid"].as_i64(),
        Some(holder as i64),
        "health named another process as the lock's holder: {health}"
    );
    let begun = Instant::now();
    kill(holder, libc::SIGTERM);
    let index = demo
        .started
        .iter()
        .position(|child| child.id() as i32 == holder)
        .unwrap();
    let status = wait_for(&mut demo.started[index]);
    assert!(wait_for(&mut waiter).success());
    assert_eq!(
        std::fs::read_to_string(dir.join("waiter")).unwrap(),
        "after\n",
        "the waiter started before the stopped build had exited"
    );
    assert!(
        !status.success(),
        "the holder exited 0 for a build that was stopped"
    );
    assert!(
        begun.elapsed() < Duration::from_secs(30),
        "the build's background sleep was not stopped"
    );
}

/// `builds health` sets up a host whose core speed it does not know, and a
/// setup replaces the host's builds and loads a launchd job under the label
/// of the person's sccache server. On the demo's host, health does neither:
/// it only reads the jobs, through the fake launchctl.
#[test]
fn health_reads_a_tests_host_without_setting_it_up_or_changing_a_launchd_job() {
    let demo = Demo::new("jobs");
    let stand_in = "#!/bin/sh\nexit 1\n";
    demo.host_m1(stand_in);
    demo.health();
    assert_eq!(
        std::fs::read(demo.dir.join("m1/.local/bin/builds")).unwrap(),
        stand_in.as_bytes(),
        "health set up host m1, replacing its builds"
    );
    let ran = demo.launchctl.ran();
    assert!(!ran.is_empty(), "health read no job through the fake");
    assert!(
        ran.iter().all(|command| command.starts_with("print ")),
        "{ran:?}"
    );
}

/// side-2, claimed and quiet for three hours with no build of its own, is
/// idle.
#[test]
fn a_side_worktree_quiet_for_hours_with_no_build_is_idle() {
    let demo = Demo::new("idle");
    assert!(
        demo.idle(&demo.health()),
        "health did not call the quiet side-2 idle"
    );
}

/// A `builds lock` run from side-2 builds for it: side-2 is not idle.
#[test]
fn a_lock_run_from_a_side_worktree_builds_for_it() {
    let mut demo = Demo::new("from");
    let dir = demo.dir.clone();
    let side = demo.side.clone();
    let holder = demo.lock(
        &side,
        r#"touch "$1"; exec sleep 30"#,
        &[path(&dir.join("started"))],
    );
    until("the build to start", || dir.join("started").exists());
    let health = demo.health();
    assert_eq!(
        demo.side_2(&health)["app_build"].as_i64(),
        Some(holder as i64),
        "health did not link the lock run from side-2 to it: {health}"
    );
    assert!(
        !demo.idle(&health),
        "health called side-2 idle while its build held the app lock"
    );
}

/// A `builds lock` run from elsewhere that moves the app checkout to side-2's
/// commit, then killed outright: its build still holds the lock, and health
/// finds that build by the open lock file and links it to side-2 through the
/// commit.
#[test]
fn a_killed_holders_build_is_found_by_the_open_lock_file_and_linked_by_commit() {
    let mut demo = Demo::new("found");
    let dir = demo.dir.clone();
    let app = demo.app.clone();
    let holder = demo.lock(
        &dir,
        r#"cd "$1" && git checkout -q --detach "$2" && touch "$3" && sleep 30"#,
        &[path(&app), demo.work.clone(), path(&dir.join("started"))],
    );
    until("the build to start", || dir.join("started").exists());
    let build = child_of(holder);
    kill(holder, libc::SIGKILL);
    let index = demo
        .started
        .iter()
        .position(|child| child.id() as i32 == holder)
        .unwrap();
    let _ = wait_for(&mut demo.started[index]);
    let health = demo.health();
    assert_eq!(
        health["locks"][0]["holder"]["pid"].as_i64(),
        Some(build as i64),
        "health did not find the killed holder's build by the open lock file: {health}"
    );
    assert_eq!(
        demo.side_2(&health)["app_build"].as_i64(),
        Some(build as i64),
        "health did not link the app checkout's build at side-2's commit to side-2"
    );
    assert!(
        !demo.idle(&health),
        "health called side-2 idle while a build at its commit held the app lock"
    );
    kill(-build, libc::SIGTERM);
}

/// A `builds cargo test` run from side-2 whose cargo runs on host m1 builds
/// for side-2, though no build process of it runs on this Mac: side-2 is
/// not idle, and health names the run's host.
#[test]
fn a_cargo_run_on_another_host_builds_for_the_worktree_it_was_run_from() {
    let mut demo = Demo::new("remote");
    // The fake cargo on m1 says it started through this FIFO, which a thread
    // here reads. The job's claim runs `builds setup` there first.
    let started = demo.dir.join("started");
    let made = Command::new("mkfifo").arg(&started).status().unwrap();
    assert!(made.success(), "mkfifo");
    demo.host_m1(&format!(
        "#!/bin/sh\n[ \"$1\" != __hold-awake ] || exec '{BUILDS}' \"$@\"\n[ \"$1\" = setup ] && exit 0\n[ \"$1\" = cargo ] || exit 1\necho started > '{}'\nexec sleep 30\n",
        started.display()
    ));
    let (said, heard) = std::sync::mpsc::channel();
    let fifo = started.clone();
    std::thread::spawn(move || {
        let _ = said.send(std::fs::read_to_string(fifo));
    });
    let run = Spawned::start(
        demo.builds()
            .current_dir(&demo.side)
            .args(["cargo", "test"])
            .env("BUILDS_CARGO_HOST", "m1")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("BUILDS_CARGO_BEFORE")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("FFMPEG_DIR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    );
    let pid = run.id() as i64;
    demo.started.push(run);
    // PATIENCE only bounds a run that never starts; the FIFO says when it does.
    match heard.recv_timeout(PATIENCE) {
        Ok(Ok(text)) => assert_eq!(text, "started\n"),
        Ok(Err(error)) => panic!("reading the FIFO: {error}"),
        Err(_) => panic!("cargo never started on m1"),
    }
    let health = demo.health();
    let side = demo.side_2(&health);
    let runs: Vec<(i64, &str)> = side["remote_runs"]
        .as_array()
        .unwrap_or_else(|| panic!("no remote runs in {side}"))
        .iter()
        .map(|run| (run["pid"].as_i64().unwrap(), run["host"].as_str().unwrap()))
        .collect();
    assert_eq!(
        runs,
        [(pid, "m1")],
        "health did not find the run on m1: {side}"
    );
    assert!(
        !demo.idle(&health),
        "health called side-2 idle while its cargo ran on m1"
    );
}
