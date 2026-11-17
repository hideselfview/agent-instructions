//! A lane's app checkout on a host, end to end: real `builds app-build`
//! processes on a scratch build volume, with a fake ssh (a program that runs
//! the command on this Mac with a second folder as the host's home) and a
//! scratch repo `demo` whose clone sits in that home. Never a real host.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

const APP_BUILD: &str = "step mac xcode ./xcode.sh\nstep android hello ./hello.sh\nstep android hang ./hang.sh\nstep android signed ./hello.sh\nhere signed\nlimit hang 1s\ntest android ./t.sh {class}\nresults android out\n";

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
        std::fs::create_dir_all(dir.join("home")).unwrap();
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
            "#!/bin/sh\nmkdir -p out\necho \"report of $1\" > out/report\necho \"ran $1\"\n[ \"$1\" != Bad ] || exit 4\n",
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
cd "$home" && HOME="$home" exec sh -c "$*"
"#,
                home = remote_home.display(),
                second = second_home.display()
            ),
        );
        std::fs::write(dir.join("hosts"), "m1 user@m1 10\n").unwrap();
        std::fs::write(dir.join("volume/demo/app-android-checkouts"), "m1:app-1\n").unwrap();
        World { dir, source }
    }

    fn app_build(&self, args: &[&str]) -> Output {
        Command::new(BUILDS)
            .arg("app-build")
            .arg("demo")
            .args(args)
            .current_dir(&self.source)
            .env("BUILDS_ROOT", self.dir.join("volume"))
            .env("HOME", self.dir.join("home"))
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env("BUILDS_LISTS", self.dir.join("lists"))
            .env("BUILDS_SSH", self.dir.join("bin/ssh"))
            .output()
            .unwrap()
    }
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
    let output = world.app_build(&["--lane", "android", "main", "--test", "android", "Bad"]);
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
    assert!(err.contains("ran past its limit of 1s"), "{err}");
    let pid = std::fs::read_to_string(world.dir.join("remote-home/hang.pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    assert_ne!(
        unsafe { libc_kill(pid) },
        0,
        "the step's process on the host is gone"
    );
}

/// `kill(pid, 0)`: 0 while the process exists.
unsafe fn libc_kill(pid: i32) -> i32 {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe { kill(pid, 0) }
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
        world.dir.join("volume/demo/app-checkouts"),
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
    assert!(err.contains("host m1's Xcode cannot start"), "{err}");
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
