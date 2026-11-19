use super::*;
use crate::host::tests::fake_ssh;
use crate::host::{Probe, Runs};
use crate::nextest::Has;
use crate::testing::{Scratch, git, repo};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// A repo `demo` with its main checkout and one side worktree.
fn checkouts(scratch: &Scratch) -> (Checkout, Checkout) {
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &side.display().to_string(),
        ],
    );
    (
        Checkout::at(&main).unwrap().unwrap(),
        Checkout::at(&side).unwrap().unwrap(),
    )
}

fn no_env(_: &str) -> Option<OsString> {
    None
}

#[test]
fn only_commands_that_compile_and_report_leave_this_mac() {
    let scratch = Scratch::new("place-rules");
    let (main, side) = checkouts(&scratch);
    let stays = |words: &[&str], checkout: &Checkout| {
        let runs = if words[0] == "nextest" {
            Runs::Nextest
        } else {
            Runs::Cargo
        };
        stays_here(&args(words), runs, Some(checkout), &no_env)
    };
    for words in [
        &["test", "-p", "demo", "--features", "test-utils"][..],
        &["clippy", "--workspace", "--", "-D", "warnings"],
        &["check", "-p", "demo"],
        &["doc", "--workspace", "--no-deps"],
        &["nextest", "run", "-p", "demo", "-E", "test(x)"],
        &["nextest", "list", "--workspace"],
        &[
            "test",
            "-p",
            "demo",
            "--",
            "--nocapture",
            "/looks/like/a/path",
        ],
    ] {
        assert_eq!(stays(words, &side), None, "{words:?}");
    }
    for (words, why) in [
        (&["build", "-p", "demo"][..], "leaves products"),
        (&["rustc", "-p", "demo", "--lib"], "leaves products"),
        (&["run", "-p", "bae-shots"], "acts on it"),
        (&["bench"], "measures it"),
        (&["install", "x"], "leaves products"),
        (&["nextest", "archive"], "leaves products"),
        (&["test", "--message-format=json"], "reads cargo's output"),
        (&["check", "--target", "aarch64-apple-ios"], "cross build"),
        (&["check", "--target=aarch64-apple-ios"], "cross build"),
        (&["test", "--release"], "cost tests"),
        (
            &["test", "--manifest-path", "x/Cargo.toml"],
            "place on this Mac",
        ),
        (&["test", "--target-dir=/tmp/t"], "place on this Mac"),
        (&["test", "/Users/x/Cargo.toml"], "path on this Mac"),
    ] {
        let said = stays(words, &side).unwrap_or_else(|| panic!("{words:?} went"));
        assert!(said.contains(why), "{words:?}: {said}");
    }
    assert!(
        stays(&["test"], &main).unwrap().contains("the person's"),
        "main stays"
    );
    assert!(
        stays_here(&args(&["test"]), Runs::Cargo, None, &no_env)
            .unwrap()
            .contains("no git checkout")
    );
    let with =
        |name: &'static str| move |asked: &str| (asked == name).then(|| OsString::from("/x"));
    let bare = |asked: &str| (asked == "RUSTC_WRAPPER").then(|| OsString::from("sccache"));
    assert_eq!(
        stays_here(&args(&["clippy"]), Runs::Cargo, Some(&side), &bare),
        None
    );
    let outside = |asked: &str| (asked == "RUSTC_WRAPPER").then(|| OsString::from("/opt/wrap.sh"));
    let said = stays_here(&args(&["clippy"]), Runs::Cargo, Some(&side), &outside).unwrap();
    assert!(
        said.starts_with("RUSTC_WRAPPER names a program outside"),
        "{said}"
    );
    let inside_path = side.top.join("scripts/wrap.sh");
    let inside = |asked: &str| {
        (asked == "RUSTC_WORKSPACE_WRAPPER").then(|| inside_path.clone().into_os_string())
    };
    assert_eq!(
        stays_here(&args(&["clippy"]), Runs::Cargo, Some(&side), &inside),
        None
    );
    for name in ["CARGO_TARGET_DIR", "FFMPEG_DIR"] {
        let said = stays_here(&args(&["clippy"]), Runs::Cargo, Some(&side), &with(name)).unwrap();
        assert!(said.starts_with(name), "{said}");
    }
}

#[test]
fn a_host_gets_the_forwarded_environment_and_runs_builds_as_local() {
    let env = |name: &str| match name {
        "RUSTDOCFLAGS" => Some(OsString::from("-D warnings")),
        "RUST_BACKTRACE" => Some(OsString::from("1")),
        "CLIPPY_ARGS" => Some(OsString::from(
            "-D__CLIPPY_HACKERY__warnings__CLIPPY_HACKERY__",
        )),
        "HOME" => Some(OsString::from("/Users/me")),
        _ => None,
    };
    let scratch = Scratch::new("place-command");
    let (_main, side) = checkouts(&scratch);
    assert_eq!(
        host_command(&args(&["doc", "--workspace"]), &side, &env),
        args(&[
            "env",
            "BUILDS_CARGO_HOST=local",
            "CLIPPY_ARGS=-D__CLIPPY_HACKERY__warnings__CLIPPY_HACKERY__",
            "RUSTDOCFLAGS=-D warnings",
            "RUST_BACKTRACE=1",
            "builds",
            "cargo",
            "doc",
            "--workspace"
        ])
    );
}

#[test]
fn a_hook_checks_run_tells_the_host_it_runs_for_a_hook() {
    let env = |name: &str| match name {
        "BUILDS_HOOK_CHECK" => Some(OsString::from("1700000000:2")),
        "BUILDS_CARGO_BEFORE" => Some(OsString::from("tools/before.sh")),
        _ => None,
    };
    let scratch = Scratch::new("place-hook");
    let (_main, side) = checkouts(&scratch);
    let words = host_command(&args(&["check"]), &side, &env);
    assert_eq!(&words[..2], &args(&["sh", "-c"]));
    assert!(
        words[2].starts_with(
            "BUILDS_HOOK_CHECK='1700000000:2'; export BUILDS_HOOK_CHECK\n\"$PWD\"/'tools/before.sh'"
        ),
        "{}",
        words[2]
    );
}

fn host(name: &str, target: Option<&str>) -> Host {
    Host {
        band: None,
        name: name.to_string(),
        target: target.map(str::to_string),
        cores: 10,
        speed: Some(1.0),
    }
}

fn probe(load: f64) -> std::result::Result<Probe, String> {
    Ok(Probe {
        memory: crate::host_memory::Memory::at_rest(),
        cores: 10,
        load: [load, load, load],
        free_kib: 1 << 30,
        git_fails: None,
        nextest: crate::nextest::Has::Nothing,
        processes: Default::default(),
    })
}

/// Starts <args> on <host> and runs it there; panics when the host refuses.
fn run_on(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    checkout: &Checkout,
    args: &[String],
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<i32> {
    match start(
        places,
        ssh,
        Choice::On(
            host.clone(),
            Box::new(probe(1.0).unwrap()),
            "named".to_string(),
        ),
        checkout,
        args,
        env,
    )? {
        Some(job) => finish(job).map(|exited| exited.code),
        None => panic!("host {} refused the job", host.name),
    }
}

struct Remote {
    scratch: Scratch,
    home: PathBuf,
    ssh: Ssh,
}

/// A fake host whose home holds a clone of `demo` (when <clone>) and a
/// `builds` that writes down how it was run.
fn remote(name: &str, clone: bool, builds_script: &str) -> (Remote, Checkout, Checkout) {
    let scratch = Scratch::new(name);
    let (main, side) = checkouts(&scratch);
    let home = scratch.dir.join("remote-home");
    std::fs::create_dir_all(home.join("dev")).unwrap();
    if clone {
        git(
            &scratch.dir,
            &[
                "clone",
                "-q",
                &main.top.display().to_string(),
                &home.join("dev/demo").display().to_string(),
            ],
        );
    }
    host_builds(&home, builds_script);
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    (Remote { scratch, home, ssh }, main, side)
}

/// Writes the host's `builds` into <home>: `builds setup`, which each job's
/// claim runs, does nothing, and every other command runs the shell lines
/// <body>.
fn host_builds(home: &Path, body: &str) {
    crate::testing::host_builds(home, &format!("[ \"$1\" = setup ] && exit 0\n{body}"));
}

const RECORDING_BUILDS: &str = r#"echo "host=$BUILDS_CARGO_HOST args=$* flags=$RUSTFLAGS" > ../ran.txt
pwd >> ../ran.txt
cat marker.txt >> ../ran.txt 2>/dev/null
exit ${EXIT:-0}
"#;

#[test]
fn the_first_host_by_room_that_has_the_clone_runs_it_and_this_mac_is_the_fallback() {
    let (r, _main, _side) = remote("place-pick", true, RECORDING_BUILDS);
    let places = r.scratch.places();
    let m1 = host("m1", Some("user@m1"));
    let no_clone = host("m2", Some("user@m2"));
    let local = host("local", None);

    // m1 ranks first and has the clone.
    let probes = vec![(local.clone(), probe(9.0)), (m1.clone(), probe(1.0))];
    let Choice::On(picked, _, why) = pick_ranked(
        &places,
        &r.ssh,
        "demo",
        &probes,
        0,
        Runs::Cargo,
        &Default::default(),
    )
    .unwrap() else {
        panic!("expected a host");
    };
    assert_eq!(picked, m1);
    assert!(
        why.starts_with(
            "m1 has 9.0 cores of room, worth 9.0 of this Mac's cores at 1.00 times its speed; local has 1.0"
        ),
        "{why}"
    );

    // This Mac ranks first: it runs here.
    let probes = vec![(local.clone(), probe(1.0)), (m1.clone(), probe(5.0))];
    assert!(matches!(
        pick_ranked(
            &places,
            &r.ssh,
            "demo",
            &probes,
            0,
            Runs::Cargo,
            &Default::default()
        )
        .unwrap(),
        Choice::Here(_)
    ));

    // A host without the clone is passed over for the next.
    std::fs::remove_dir_all(r.home.join("dev/demo")).unwrap();
    let probes = vec![(local.clone(), probe(6.0)), (no_clone.clone(), probe(0.0))];
    let Choice::Here(why) = pick_ranked(
        &places,
        &r.ssh,
        "demo",
        &probes,
        0,
        Runs::Cargo,
        &Default::default(),
    )
    .unwrap() else {
        panic!("expected this Mac");
    };
    assert!(why.contains("m2 cannot build demo"), "{why}");

    // An unreachable host is skipped and named.
    let gone = host("gone", Some("unreachable"));
    let probes = host::probe_all(&places, &r.ssh, vec![local, gone]);
    let Choice::Here(why) = pick_ranked(
        &places,
        &r.ssh,
        "demo",
        &probes,
        0,
        Runs::Cargo,
        &Default::default(),
    )
    .unwrap() else {
        panic!("expected this Mac");
    };
    assert!(why.contains("gone is unreachable"), "{why}");
}

#[test]
fn a_named_host_must_answer_and_this_mac_can_be_named() {
    let (r, _main, _side) = remote("place-named", true, RECORDING_BUILDS);
    let places = r.scratch.places();
    // No other host is recorded.
    assert!(matches!(
        choose(&places, &r.ssh, 0, "demo", None, Runs::Cargo, 1_800_000_000).unwrap(),
        Choice::Here(_)
    ));
    let file = host::hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "gone unreachable 4\nm1 user@m1 10\n").unwrap();
    let Err(crate::fail::Fail::Message(message)) = choose(
        &places,
        &r.ssh,
        0,
        "demo",
        Some("gone"),
        Runs::Cargo,
        1_800_000_000,
    ) else {
        panic!("expected a failure");
    };
    assert!(message.contains("host gone is not reachable"), "{message}");
    assert!(matches!(
        choose(
            &places,
            &r.ssh,
            0,
            "demo",
            Some("local"),
            Runs::Cargo,
            1_800_000_000
        )
        .unwrap(),
        Choice::Here(_)
    ));
    assert!(matches!(
        choose(
            &places,
            &r.ssh,
            0,
            "demo",
            Some("m1"),
            Runs::Cargo,
            1_800_000_000
        )
        .unwrap(),
        Choice::On(..)
    ));
    assert!(
        choose(
            &places,
            &r.ssh,
            0,
            "demo",
            Some("nobody"),
            Runs::Cargo,
            1_800_000_000
        )
        .is_err()
    );
}

/// A nextest run goes to a host that has the pinned cargo-nextest, which the
/// probe finds by asking the host, and a host without it is passed over, and
/// fails when `BUILDS_CARGO_HOST` names it.
#[test]
fn a_nextest_run_goes_only_to_a_host_with_cargo_nextest() {
    let (r, _main, _side) = remote("place-nextest", true, RECORDING_BUILDS);
    let places = r.scratch.places();
    let file = host::hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 user@m1 10 1.0\n").unwrap();
    let m1 = host("m1", Some("user@m1"));
    let local = host("local", None);
    let ask = || host::probe_all(&places, &r.ssh, vec![local.clone(), m1.clone()]);
    let with_local_load = |probes: Vec<(Host, std::result::Result<Probe, String>)>| {
        // This Mac's probe reads this Mac's own load, so the test gives it a
        // load of 1000 and m1 has more room however busy this Mac is.
        vec![(local.clone(), probe(1000.0)), probes[1].clone()]
    };

    let probes = ask();
    assert_eq!(probes[1].1.as_ref().unwrap().nextest, Has::Nothing);
    let Choice::Here(why) = pick_ranked(
        &places,
        &r.ssh,
        "demo",
        &with_local_load(probes.clone()),
        0,
        Runs::Nextest,
        &Default::default(),
    )
    .unwrap() else {
        panic!("expected this Mac");
    };
    assert!(why.contains("m1 has no cargo-nextest"), "{why}");
    assert!(matches!(
        pick_ranked(
            &places,
            &r.ssh,
            "demo",
            &with_local_load(probes),
            0,
            Runs::Cargo,
            &Default::default()
        )
        .unwrap(),
        Choice::On(..)
    ));
    let Err(crate::fail::Fail::Message(message)) = choose(
        &places,
        &r.ssh,
        0,
        "demo",
        Some("m1"),
        Runs::Nextest,
        1_800_000_000,
    ) else {
        panic!("expected a failure");
    };
    assert!(message.contains("m1 has no cargo-nextest"), "{message}");

    // The host gets the pinned version and takes nextest runs.
    crate::nextest::tests::fake_program(&r.home.join(".cargo/bin"), crate::nextest::VERSION);
    let probes = ask();
    assert_eq!(
        probes[1].1.as_ref().unwrap().nextest,
        Has::Version(crate::nextest::VERSION.to_string())
    );
    assert!(matches!(
        pick_ranked(
            &places,
            &r.ssh,
            "demo",
            &with_local_load(probes),
            0,
            Runs::Nextest,
            &Default::default()
        )
        .unwrap(),
        Choice::On(..)
    ));
    assert!(matches!(
        choose(
            &places,
            &r.ssh,
            0,
            "demo",
            Some("m1"),
            Runs::Nextest,
            1_800_000_000
        )
        .unwrap(),
        Choice::On(..)
    ));
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn a_host_builds_the_working_tree_and_its_exit_status_comes_back() {
    let (r, _main, side) = remote("place-run", true, RECORDING_BUILDS);
    let places = r.scratch.places();
    let m1 = host("m1", Some("user@m1"));
    std::fs::write(side.top.join("marker.txt"), "an untracked edit\n").unwrap();
    let env = |name: &str| (name == "RUSTFLAGS").then(|| OsString::from("-Dx"));
    let code = run_on(
        &places,
        &r.ssh,
        &m1,
        &side,
        &args(&["test", "-p", "demo"]),
        &env,
    )
    .unwrap();
    assert_eq!(code, 0);
    let ran = read(&r.home.join("dev/demo-worktrees/ran.txt"));
    assert!(
        ran.starts_with("host=local args=cargo test -p demo flags=-Dx\n"),
        "{ran}"
    );
    assert!(ran.contains("demo-worktrees/remote-side-1"), "{ran}");
    assert!(ran.ends_with("an untracked edit\n"), "{ran}");

    // A failing command fails the same way.
    host_builds(&r.home, "exit 101\n");
    assert_eq!(
        run_on(&places, &r.ssh, &m1, &side, &args(&["test"]), &no_env).unwrap(),
        101
    );
}

#[test]
fn a_host_that_drops_fails_loudly_naming_it() {
    let (r, _main, side) = remote("place-drop", true, "exit 255\n");
    let places = r.scratch.places();
    let m1 = host("m1", Some("user@m1"));
    let Err(crate::fail::Fail::Message(message)) =
        run_on(&places, &r.ssh, &m1, &side, &args(&["test"]), &no_env)
    else {
        panic!("expected a failure");
    };
    assert!(message.contains("host m1 stopped answering"), "{message}");
    assert!(
        message.contains("nothing ran the command again"),
        "{message}"
    );
}

#[test]
fn a_wrapper_and_a_before_program_inside_the_checkout_run_from_the_hosts_worktree() {
    let (r, _main, side) = remote("place-wrapper", true, RECORDING_BUILDS);
    let places = r.scratch.places();
    let m1 = host("m1", Some("user@m1"));
    let wrapper = side.top.join("scripts/wrap.sh");
    let env = |name: &str| match name {
        "RUSTC_WRAPPER" => Some(wrapper.clone().into_os_string()),
        "BUILDS_CARGO_BEFORE" => Some(OsString::from("scripts/before.sh one two")),
        _ => None,
    };
    let words = host_command(&args(&["clippy", "-p", "demo"]), &side, &env);
    assert_eq!(&words[..2], ["sh", "-c"]);
    assert!(
        words[2].starts_with("\"$PWD\"/'scripts/before.sh' 'one' 'two' || exit $?\n"),
        "{}",
        words[2]
    );
    assert!(
        words[2].contains("RUSTC_WRAPPER=\"$PWD\"/'scripts/wrap.sh'"),
        "{}",
        words[2]
    );
    // The fake `builds` writes down the wrapper it got; the before program
    // (a file of the checkout, so also in the host's clone) writes its own
    // arguments and the folder it ran in, before cargo.
    host_builds(
        &r.home,
        "echo \"$RUSTC_WRAPPER\" > ../wrapper.txt\ncat ../before.txt > ../seen-before.txt\n",
    );
    let script = side.top.join("scripts/before.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"$* $(basename \"$PWD\")\" > ../before.txt\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let code = run_on(
        &places,
        &r.ssh,
        &m1,
        &side,
        &args(&["clippy", "-p", "demo"]),
        &env,
    )
    .unwrap();
    assert_eq!(code, 0);
    let wts = r.home.join("dev/demo-worktrees");
    assert_eq!(
        read(&wts.join("wrapper.txt")).trim(),
        format!("{}/remote-side-1/scripts/wrap.sh", wts.display())
    );
    assert_eq!(
        read(&wts.join("seen-before.txt")).trim(),
        "one two remote-side-1"
    );

    // A program that fails stops the command before cargo, with its status.
    std::fs::write(&script, "#!/bin/sh\nexit 7\n").unwrap();
    std::fs::remove_file(wts.join("wrapper.txt")).unwrap();
    assert_eq!(
        run_on(&places, &r.ssh, &m1, &side, &args(&["clippy"]), &env).unwrap(),
        7
    );
    assert!(!wts.join("wrapper.txt").exists(), "cargo did not run");
}

#[test]
fn a_before_program_outside_the_checkout_keeps_the_command_here_and_runs_here() {
    let scratch = Scratch::new("place-before");
    let (_main, side) = checkouts(&scratch);
    let outside = |name: &str| (name == "BUILDS_CARGO_BEFORE").then(|| OsString::from("/opt/x.sh"));
    let said = stays_here(&args(&["clippy"]), Runs::Cargo, Some(&side), &outside).unwrap();
    assert!(
        said.starts_with("BUILDS_CARGO_BEFORE names a program outside"),
        "{said}"
    );
    let escapes = |name: &str| (name == "BUILDS_CARGO_BEFORE").then(|| OsString::from("../x.sh"));
    assert!(stays_here(&args(&["clippy"]), Runs::Cargo, Some(&side), &escapes).is_some());
    let inside =
        |name: &str| (name == "BUILDS_CARGO_BEFORE").then(|| OsString::from("scripts/x.sh a"));
    assert_eq!(
        stays_here(&args(&["clippy"]), Runs::Cargo, Some(&side), &inside),
        None
    );

    // Here, the program runs in the checkout's top.
    let script = side.top.join("scripts/before.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "#!/bin/sh\necho \"$1\" > ran-here.txt\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    Before::from_env(&|_| Some(OsString::from("scripts/before.sh hello")))
        .unwrap()
        .run_here(&side)
        .unwrap();
    assert_eq!(read(&side.top.join("ran-here.txt")).trim(), "hello");
    assert!(
        Before::from_env(&|_| Some(OsString::from("scripts/missing.sh")))
            .unwrap()
            .run_here(&side)
            .is_err()
    );
}

/// An ssh that reaches the fake host's home with a `git` and a
/// `git-receive-pack` that fail as git does on a Mac whose Xcode license is
/// not accepted, while the file `refuses` is in <home>.
fn refusing_ssh(r: &Remote) -> Ssh {
    let bin = r.scratch.dir.join("refusing-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let git = String::from_utf8(
        std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    for name in ["git", "git-receive-pack"] {
        let program = bin.join(name);
        let real_program = if name == "git" {
            git.trim().to_string()
        } else {
            format!("{} receive-pack", git.trim())
        };
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nif [ -e '{home}/refuses' ]; then\n  echo \"You have not agreed to the Xcode license agreements. Please run 'sudo xcodebuild -license'.\" >&2\n  exit 69\nfi\nexec {real_program} \"$@\"\n",
                home = r.home.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let wrapper = r.scratch.dir.join("refusing-ssh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nPATH='{bin}':\"$PATH\" exec '{ssh}' \"$@\"\n",
            bin = bin.display(),
            ssh = r.ssh.program
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    Ssh {
        program: wrapper.display().to_string(),
    }
}

#[test]
fn a_host_that_refuses_the_job_leaves_it_to_this_mac_and_is_passed_over_until_git_runs() {
    let (r, _main, side) = remote("place-refuses", true, RECORDING_BUILDS);
    let places = r.scratch.places();
    let file = host::hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 user@m1 10\n").unwrap();
    let ssh = refusing_ssh(&r);
    let m1 = host("m1", Some("user@m1"));
    std::fs::write(r.home.join("refuses"), "").unwrap();

    // The push of the commit fails on the host: the command runs here.
    let job = start(
        &places,
        &ssh,
        Choice::On(
            m1.clone(),
            Box::new(probe(1.0).unwrap()),
            "most room".to_string(),
        ),
        &side,
        &args(&["test"]),
        &no_env,
    )
    .unwrap();
    assert!(job.is_none());
    assert!(!r.home.join("dev/demo-worktrees/ran.txt").exists());
    let refusals = host::refusals(&places).unwrap();
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].host, "m1");
    assert!(
        refusals[0]
            .said
            .contains("You have not agreed to the Xcode license agreements"),
        "{:?}",
        refusals[0]
    );
    let finding = host::refusal_finding(&refusals[0]);
    assert!(
        finding.text.starts_with("host m1 refuses jobs (since ")
            && finding.text.contains("sudo xcodebuild -license")
            && finding.text.contains("Fix what m1 said on m1")
            && !finding.text.contains('`'),
        "{finding:?}"
    );
    assert_eq!(finding.command.as_deref(), Some("builds host list"));

    // A pick probes the host, finds git failing there and passes it over, so
    // the command runs here even though m1 has the most room. m1's probe
    // reads this Mac's own cores and load through the stand-in ssh, so the
    // test gives this Mac's probe a load of 1000, higher than any real Mac
    // reports, and m1 has more room however busy this Mac is.
    let local = host("local", None);
    let probes = host::probe_all(&places, &ssh, vec![local.clone(), m1.clone()]);
    assert!(probes[1].1.as_ref().unwrap().git_fails.is_some());
    let probes: Vec<_> = vec![(local.clone(), probe(1000.0)), probes[1].clone()];
    let Choice::Here(why) = pick_ranked(
        &places,
        &ssh,
        "demo",
        &probes,
        0,
        Runs::Cargo,
        &Default::default(),
    )
    .unwrap() else {
        panic!("expected this Mac");
    };
    assert!(
        why.contains("m1 refuses jobs (git fails there: You have not agreed"),
        "{why}"
    );
    assert_eq!(host::refusals(&places).unwrap().len(), 1);

    // Once git runs there, the probe removes the record and m1 takes jobs.
    std::fs::remove_file(r.home.join("refuses")).unwrap();
    let probes = host::probe_all(&places, &ssh, vec![m1.clone()]);
    assert_eq!(probes[0].1.as_ref().unwrap().git_fails, None);
    assert!(host::refusals(&places).unwrap().is_empty());
    let probes = vec![(local, probe(1000.0)), probes[0].clone()];
    assert!(matches!(
        pick_ranked(
            &places,
            &ssh,
            "demo",
            &probes,
            0,
            Runs::Cargo,
            &Default::default()
        )
        .unwrap(),
        Choice::On(..)
    ));
    assert_eq!(
        run_on(&places, &ssh, &m1, &side, &args(&["test"]), &no_env).unwrap(),
        0
    );

    // A host gone before the send refuses the same way.
    let gone = host("gone", Some("unreachable"));
    let job = start(
        &places,
        &ssh,
        Choice::On(gone, Box::new(probe(1.0).unwrap()), "most room".to_string()),
        &side,
        &args(&["test"]),
        &no_env,
    )
    .unwrap();
    assert!(job.is_none());
}

#[test]
fn an_unset_or_empty_host_variable_names_no_host_and_one_not_utf8_fails() {
    use std::os::unix::ffi::OsStringExt;
    let reading = |value: Option<OsString>| {
        move |name: &str| {
            assert_eq!(name, HOST_VARIABLE);
            value.clone()
        }
    };
    assert_eq!(named_host(&reading(None)).unwrap(), None);
    assert_eq!(named_host(&reading(Some("".into()))).unwrap(), None);
    assert_eq!(
        named_host(&reading(Some("m1".into()))).unwrap(),
        Some("m1".to_string())
    );
    let said = named_host(&reading(Some(OsString::from_vec(vec![0x6d, 0xff]))))
        .unwrap_err()
        .text();
    assert!(said.contains("not UTF-8"), "{said}");
}

#[test]
fn only_a_named_other_host_gets_the_note_that_the_command_runs_here() {
    assert_eq!(runs_here_note(None, "why"), None);
    assert_eq!(runs_here_note(Some(host::LOCAL), "why"), None);
    assert_eq!(
        runs_here_note(Some("m1"), "why").unwrap(),
        format!("{HOST_VARIABLE} names host m1; this command runs on this Mac: why")
    );
}
