use super::*;
use crate::host::tests::fake_ssh;
use crate::testing::{Scratch, git};

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// A source repo `demo` with one commit, and a remote home holding a clone of
/// it at dev/demo.
struct Setup {
    scratch: Scratch,
    source: PathBuf,
    home: PathBuf,
    ssh: Ssh,
    host: Host,
}

fn setup(name: &str) -> Setup {
    let scratch = Scratch::new(name);
    let source = scratch.dir.join("source/demo");
    crate::testing::repo(&source, "target 1G\n");
    let home = scratch.dir.join("remote-home");
    std::fs::create_dir_all(home.join("dev")).unwrap();
    git(
        &scratch.dir,
        &[
            "clone",
            "-q",
            &source.display().to_string(),
            &home.join("dev/demo").display().to_string(),
        ],
    );
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
    let host = Host {
        name: "m1".to_string(),
        target: Some("user@m1".to_string()),
        cores: 10,
    };
    Setup {
        scratch,
        source,
        home,
        ssh,
        host,
    }
}

fn request(what: What, command: &[&str]) -> Request {
    Request {
        repo: "demo".to_string(),
        what,
        host: None,
        command: args(command),
    }
}

fn refs(repo: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["for-each-ref", "refs/builds"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn parses_a_request() {
    let parsed = parse(&args(&[
        "run",
        "bae",
        "origin/main",
        "--host",
        "m1",
        "--",
        "ls",
        "-l",
    ]))
    .unwrap();
    assert_eq!(
        parsed,
        Request {
            repo: "bae".to_string(),
            what: What::Ref("origin/main".to_string()),
            host: Some("m1".to_string()),
            command: args(&["ls", "-l"]),
        }
    );
    let staged = parse(&args(&["run", "bae", "--staged", "--", "true"])).unwrap();
    assert_eq!(staged.what, What::Staged);
    assert!(
        parse(&args(&["run", "bae", "--", "true"])).is_err(),
        "no ref"
    );
    assert!(parse(&args(&["run", "bae", "main"])).is_err(), "no command");
    assert!(parse(&args(&["run", "bae", "main", "--",])).is_err());
}

#[test]
fn runs_a_commit_in_a_worktree_and_cleans_up() {
    let s = setup("remote-run");
    let places = s.scratch.places();
    let sha = commit_of(&s.source, &What::Ref("HEAD".to_string())).unwrap();
    let job = request(
        What::Ref("main".to_string()),
        &[
            "sh",
            "-c",
            "git rev-parse HEAD > ../ran.txt; pwd > ../where.txt; exit 7",
        ],
    );
    let code = run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap();
    assert_eq!(code, 7, "the command's exit status comes back");
    let wts = s.home.join("dev/demo-worktrees");
    assert_eq!(
        std::fs::read_to_string(wts.join("ran.txt")).unwrap().trim(),
        sha
    );
    assert!(
        std::fs::read_to_string(wts.join("where.txt"))
            .unwrap()
            .trim()
            .ends_with("demo-worktrees/remote-1")
    );
    assert!(!wts.join("remote-1.job").exists(), "the claim is released");
    assert_eq!(
        refs(&s.home.join("dev/demo")),
        "",
        "the job's ref is deleted"
    );
    assert_eq!(refs(&s.source), "", "nothing is left in the source");
    // A second job reuses the free worktree.
    run_job(
        &places,
        &s.ssh,
        &s.host,
        &s.source,
        &request(What::Ref("main".to_string()), &["true"]),
    )
    .unwrap();
    assert!(!wts.join("remote-2").exists());
}

#[test]
fn a_busy_worktree_sends_the_next_job_to_another() {
    let s = setup("remote-busy");
    let places = s.scratch.places();
    let wts = s.home.join("dev/demo-worktrees");
    std::fs::create_dir_all(wts.join("remote-1.job")).unwrap();
    let job = request(
        What::Ref("main".to_string()),
        &["sh", "-c", "pwd > ../where.txt"],
    );
    run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap();
    assert!(
        std::fs::read_to_string(wts.join("where.txt"))
            .unwrap()
            .trim()
            .ends_with("remote-2")
    );
    assert!(
        wts.join("remote-1.job").exists(),
        "another job's claim stays"
    );
}

#[test]
fn sends_the_staged_files_without_moving_a_ref() {
    let s = setup("remote-staged");
    let places = s.scratch.places();
    std::fs::write(s.source.join("new.txt"), "staged\n").unwrap();
    git(&s.source, &["add", "new.txt"]);
    std::fs::write(s.source.join("unstaged.txt"), "no\n").unwrap();
    let head_before = commit_of(&s.source, &What::Ref("HEAD".to_string())).unwrap();
    let job = request(
        What::Staged,
        &[
            "sh",
            "-c",
            "cat new.txt > ../out.txt; ls unstaged.txt 2>/dev/null > ../unstaged.txt || true",
        ],
    );
    assert_eq!(
        run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap(),
        0
    );
    let wts = s.home.join("dev/demo-worktrees");
    assert_eq!(
        std::fs::read_to_string(wts.join("out.txt")).unwrap(),
        "staged\n"
    );
    assert_eq!(
        std::fs::read_to_string(wts.join("unstaged.txt")).unwrap(),
        ""
    );
    assert_eq!(
        commit_of(&s.source, &What::Ref("HEAD".to_string())).unwrap(),
        head_before
    );
    assert_eq!(refs(&s.source), "");
    assert_eq!(refs(&s.home.join("dev/demo")), "");
}

#[test]
fn an_unreachable_named_host_fails_naming_it() {
    let s = setup("remote-gone");
    let places = s.scratch.places();
    let remote_hosts = host::hosts_file(&places);
    std::fs::create_dir_all(remote_hosts.parent().unwrap()).unwrap();
    std::fs::write(&remote_hosts, "gone unreachable 4\nm1 user@m1 10\n").unwrap();
    let Err(Fail::Message(message)) = pick_host(&places, &s.ssh, 0, Some("gone")) else {
        panic!("expected a failure");
    };
    assert!(message.contains("host gone is not reachable"), "{message}");
    assert!(message.contains("Connection refused"), "{message}");
    let (picked, why) = pick_host(&places, &s.ssh, 0, Some("m1")).unwrap();
    assert_eq!(picked.name, "m1");
    assert!(why.contains("named with --host"));
    // Unnamed, the unreachable host is skipped and the line says so.
    let (_, why) = pick_host(&places, &s.ssh, 0, None).unwrap();
    assert!(why.contains("gone is unreachable"), "{why}");
}

#[test]
fn a_host_without_a_clone_fails_loudly_and_leaves_nothing() {
    let s = setup("remote-noclone");
    let places = s.scratch.places();
    std::fs::remove_dir_all(s.home.join("dev/demo")).unwrap();
    let job = request(What::Ref("main".to_string()), &["true"]);
    let Err(Fail::Message(message)) = run_job(&places, &s.ssh, &s.host, &s.source, &job) else {
        panic!("expected a failure");
    };
    assert!(message.contains("host m1"), "{message}");
    assert_eq!(refs(&s.source), "");
}

#[test]
fn this_mac_runs_a_job_without_pushing() {
    let s = setup("remote-local");
    let mut places = s.scratch.places();
    places.home = s.scratch.dir.join("mac-home");
    std::fs::create_dir_all(places.home.join("dev")).unwrap();
    std::os::unix::fs::symlink(&s.source, places.home.join("dev/demo")).unwrap();
    let job = request(
        What::Ref("main".to_string()),
        &["sh", "-c", "pwd > ../where.txt; exit 3"],
    );
    let code = run_job(&places, &s.ssh, &Host::local(), &s.source, &job).unwrap();
    assert_eq!(code, 3);
    let wts = places.home.join("dev/demo-worktrees");
    assert!(
        std::fs::read_to_string(wts.join("where.txt"))
            .unwrap()
            .trim()
            .ends_with("remote-1")
    );
    assert!(!wts.join("remote-1.job").exists());
    assert_eq!(refs(&s.source), "");
}

#[test]
fn a_stop_signal_stops_the_command_on_the_host() {
    let s = setup("remote-stop");
    let places = s.scratch.places();
    let marker = s.home.join("started");
    let job = request(
        What::Ref("main".to_string()),
        &[
            "sh",
            "-c",
            &format!("touch {}; exec tail -f /dev/null", marker.display()),
        ],
    );
    let waiting = marker.clone();
    let signaller = std::thread::spawn(move || {
        while !waiting.exists() {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Give run_command a moment to record the pid, then stop.
        std::thread::sleep(Duration::from_millis(300));
        note_stop(0);
    });
    let code = run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap();
    signaller.join().unwrap();
    assert_eq!(code, STOPPED_EXIT);
    assert!(!s.home.join("dev/demo-worktrees/remote-1.job").exists());
    assert_eq!(refs(&s.home.join("dev/demo")), "");
}
