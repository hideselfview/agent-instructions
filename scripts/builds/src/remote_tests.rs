use super::*;
use crate::host::tests::fake_ssh;
use crate::host_platform::Os;
use crate::testing::{Scratch, fifo, git, gone_pid, host_builds};

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
    fake_builds(&home);
    let ssh = fake_ssh(&scratch, &home);
    let host = Host {
        band: None,
        name: "m1".to_string(),
        target: Some("user@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
    };
    Setup {
        scratch,
        source,
        home,
        ssh,
        host,
    }
}

/// Puts a `builds` where host setup installs it in <home>, which writes each
/// run's arguments to <home>/builds-runs.txt and exits with status 0.
fn fake_builds(home: &Path) {
    host_builds(home, "echo \"builds $*\" >> \"$HOME/builds-runs.txt\"");
}

fn request(what: What, command: &[&str]) -> Request {
    Request {
        repo: "demo".to_string(),
        what,
        host: None,
        command: args(command),
        prepare: crate::setup::Prepare::Yes,
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
    let parsed = parse(
        &crate::args::Args::of(
            crate::command::Command::Remote(crate::command::remote::Run),
            &args(&["bae", "origin/main", "--host", "m1", "--", "ls", "-l"]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        parsed,
        Request {
            repo: "bae".to_string(),
            what: What::Ref("origin/main".to_string()),
            host: Some("m1".to_string()),
            command: args(&["ls", "-l"]),
            prepare: crate::setup::Prepare::Yes,
        }
    );
    let staged = parse(
        &crate::args::Args::of(
            crate::command::Command::Remote(crate::command::remote::Run),
            &args(&["bae", "--staged", "--", "true"]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(staged.what, What::Staged);
    let working = parse(
        &crate::args::Args::of(
            crate::command::Command::Remote(crate::command::remote::Run),
            &args(&["bae", "--working", "--", "true"]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(working.what, What::Working);
    assert!(
        crate::args::Args::of(
            crate::command::Command::Remote(crate::command::remote::Run),
            &args(&["bae", "--", "true"])
        )
        .and_then(|args| parse(&args).map(drop))
        .is_err(),
        "no ref"
    );
    assert!(
        crate::args::Args::of(
            crate::command::Command::Remote(crate::command::remote::Run),
            &args(&["bae", "main"])
        )
        .and_then(|args| parse(&args).map(drop))
        .is_err(),
        "no command"
    );
    assert!(
        crate::args::Args::of(
            crate::command::Command::Remote(crate::command::remote::Run),
            &args(&["bae", "main", "--",])
        )
        .and_then(|args| parse(&args).map(drop))
        .is_err()
    );
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
            .ends_with("demo-worktrees/remote-demo"),
        "the job runs in the worktree of the checkout it came from"
    );
    assert!(
        !wts.join("remote-demo.job").exists(),
        "run_job releases the claim"
    );
    assert_eq!(
        refs(&s.home.join("dev/demo")),
        "",
        "the job's ref is deleted"
    );
    assert_eq!(refs(&s.source), "", "nothing is left in the source");
    // A second job from the checkout reuses its worktree.
    run_job(
        &places,
        &s.ssh,
        &s.host,
        &s.source,
        &request(What::Ref("main".to_string()), &["true"]),
    )
    .unwrap();
    assert!(!wts.join("remote-demo.2").exists());
}

#[test]
fn a_busy_worktree_sends_the_next_job_to_another() {
    let s = setup("remote-busy");
    let places = s.scratch.places();
    let wts = s.home.join("dev/demo-worktrees");
    std::fs::create_dir_all(wts.join("remote-demo.job")).unwrap();
    let job = request(
        What::Ref("main".to_string()),
        &["sh", "-c", "pwd > ../where.txt"],
    );
    run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap();
    assert!(
        std::fs::read_to_string(wts.join("where.txt"))
            .unwrap()
            .trim()
            .ends_with("remote-demo.2")
    );
    assert!(
        wts.join("remote-demo.job").exists(),
        "another job's claim stays"
    );
}

/// A claim whose builds process is gone holds nothing: the next job from the
/// checkout removes it and runs in the checkout's worktree. A claim whose
/// process runs still sends the job to another worktree.
#[test]
fn the_next_job_takes_over_a_claim_whose_process_is_gone() {
    let s = setup("remote-dead-claim");
    let places = s.scratch.places();
    let wts = s.home.join("dev/demo-worktrees");
    let job = || {
        request(
            What::Ref("main".to_string()),
            &["sh", "-c", "pwd > ../where.txt"],
        )
    };
    run_job(&places, &s.ssh, &s.host, &s.source, &job()).unwrap();
    let claim = wts.join("remote-demo.job");
    std::fs::create_dir_all(&claim).unwrap();
    let dead = format!("{}-{}", gone_pid(), job_id().split_once('-').unwrap().1);
    std::fs::write(claim.join("id"), format!("{dead}\n")).unwrap();

    run_job(&places, &s.ssh, &s.host, &s.source, &job()).unwrap();
    let ran_in = std::fs::read_to_string(wts.join("where.txt")).unwrap();
    assert!(ran_in.trim().ends_with("remote-demo"), "{ran_in}");
    assert!(!claim.exists(), "the job released the claim it took over");
    assert!(!wts.join("remote-demo.2").exists());

    std::fs::create_dir_all(&claim).unwrap();
    std::fs::write(claim.join("id"), format!("{}\n", job_id())).unwrap();
    run_job(&places, &s.ssh, &s.host, &s.source, &job()).unwrap();
    let ran_in = std::fs::read_to_string(wts.join("where.txt")).unwrap();
    assert!(ran_in.trim().ends_with("remote-demo.2"), "{ran_in}");
    assert!(claim.join("id").exists(), "a running job's claim stays");
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
    let picked = pick_host(&places, &s.ssh, 0, Some("m1")).unwrap();
    assert_eq!(picked.host.name, "m1");
    assert!(picked.why.contains("named with --host"));
    // Unnamed, the unreachable host is skipped and the line says so.
    let picked = pick_host(&places, &s.ssh, 0, None).unwrap();
    assert!(picked.why.contains("gone is unreachable"), "{}", picked.why);
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
    fake_builds(&places.home);
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
            .ends_with("remote-demo")
    );
    assert!(!wts.join("remote-demo.job").exists());
    assert_eq!(refs(&s.source), "");
}

#[test]
fn a_host_that_cannot_stay_awake_runs_no_command() {
    let s = setup("remote-no-hold");
    let places = s.scratch.places();
    // A builds that sets the worktree up and holds no assertion.
    std::fs::write(
        s.home.join(".local/bin/builds"),
        "#!/bin/sh\n[ \"$1\" = setup ] && exit 0\nexit 1\n",
    )
    .unwrap();
    let job = request(
        What::Ref("main".to_string()),
        &["sh", "-c", "echo > ../ran"],
    );
    let code = run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap();
    assert_eq!(code, 98);
    let wts = s.home.join("dev/demo-worktrees");
    assert!(!wts.join("ran").exists(), "the command ran");
    assert!(!wts.join("remote-demo.job").exists());
}

/// Sends SIGTERM to this test's own process, as a person or a parent stops
/// `builds`.
fn stop_this_process() {
    stop_on_signals();
    unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
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
            &format!("echo > {}; exec tail -f /dev/null", marker.display()),
        ],
    );
    fifo(&marker);
    let signaller = std::thread::spawn(move || {
        // The read waits until the command has started.
        std::fs::read_to_string(&marker).unwrap();
        stop_this_process();
    });
    let code = run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap();
    signaller.join().unwrap();
    assert_eq!(code, STOPPED_EXIT);
    assert!(!s.home.join("dev/demo-worktrees/remote-demo.job").exists());
    assert_eq!(refs(&s.home.join("dev/demo")), "");
}

#[test]
fn a_stop_before_the_command_starts_stops_it() {
    let s = setup("remote-stop-early");
    let places = s.scratch.places();
    let wt = app_checkout(&s);
    let transport = Transport {
        places: &places,
        ssh: &s.ssh,
        host: &s.host,
    };
    // The stop comes before the step starts, as one during the send or the
    // claim does.
    stop_this_process();
    let started = std::time::Instant::now();
    let ran = run_command(
        &transport,
        &wt.display().to_string(),
        &args(&["sleep", "10"]),
        None,
    );
    let took = started.elapsed();
    assert_eq!(ran.unwrap(), (STOPPED_EXIT, None));
    assert!(
        took < Duration::from_secs(5),
        "the stop stops the step: {took:?}"
    );
}

#[test]
fn a_stop_signal_stops_every_command_the_process_runs() {
    let s = setup("remote-stop-both");
    let places = s.scratch.places();
    let worktrees = [app_checkout(&s), s.home.join("dev/demo-worktrees/app-2")];
    std::fs::create_dir_all(&worktrees[1]).unwrap();
    std::fs::create_dir_all(format!("{}.job", worktrees[1].display())).unwrap();
    for wt in &worktrees {
        fifo(&wt.join("started"));
    }
    let transport = Transport {
        places: &places,
        ssh: &s.ssh,
        host: &s.host,
    };
    std::thread::scope(|scope| {
        let (sender, ran) = std::sync::mpsc::channel();
        for wt in &worktrees {
            let (sender, transport) = (sender.clone(), &transport);
            scope.spawn(move || {
                let code = run_command(
                    transport,
                    &wt.display().to_string(),
                    &args(&["sh", "-c", "echo $$ > started; exec sleep 600"]),
                    None,
                );
                sender.send(code.unwrap().0).unwrap();
            });
        }
        // Each read waits until its command has started and written its
        // process group.
        let groups: Vec<i32> = worktrees
            .iter()
            .map(|wt| {
                std::fs::read_to_string(wt.join("started"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap()
            })
            .collect();
        stop_this_process();
        let codes: Vec<Option<i32>> = groups
            .iter()
            .map(|_| ran.recv_timeout(Duration::from_secs(10)).ok())
            .collect();
        // A command the stop missed would keep its thread, and the scope,
        // from ending.
        for group in &groups {
            if crate::procs::runs(-group) {
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
        }
        assert_eq!(codes, [Some(STOPPED_EXIT), Some(STOPPED_EXIT)]);
    });
}

/// The variables that hand `killed_caller` the fake ssh and the worktree.
const CALLER_SSH: &str = "BUILDS_TEST_CALLER_SSH";
const CALLER_WORKTREE: &str = "BUILDS_TEST_CALLER_WORKTREE";

/// The side of a job on this Mac, run by
/// `a_killed_caller_stops_the_command_on_the_host` in a process of its own,
/// which that test kills. The command opens the FIFO `alive` in the worktree
/// for writing and keeps it open, writes its process group's id to the FIFO
/// `started` there, and runs until something stops it. A plain run of the
/// suite finds no environment and does nothing.
#[test]
fn killed_caller() {
    let (Some(ssh), Some(worktree)) = (
        std::env::var(CALLER_SSH).ok(),
        std::env::var(CALLER_WORKTREE).ok(),
    ) else {
        return;
    };
    let places = Places::from_env().unwrap();
    let host = Host {
        band: None,
        name: "m1".to_string(),
        target: Some("user@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
    };
    let transport = Transport {
        places: &places,
        ssh: &crate::testing::ssh_at(std::path::Path::new(&ssh)),
        host: &host,
    };
    // As `builds cargo` runs it: builds reads the command's stderr.
    run_command(
        &transport,
        &worktree,
        &args(&[
            "sh",
            "-c",
            "exec 6>alive; echo $$ > started; exec sleep 600",
        ]),
        Some(&crate::cargo_run_outcome::Watch::default()),
    )
    .unwrap();
}

#[test]
fn a_killed_caller_stops_the_command_on_the_host() {
    let s = setup("remote-killed-caller");
    let wt = app_checkout(&s);
    let (started, alive) = (wt.join("started"), wt.join("alive"));
    fifo(&started);
    fifo(&alive);
    let mut caller = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "remote::tests::killed_caller",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(
            CALLER_SSH,
            s.ssh.system().path(crate::system::Program::Ssh).unwrap(),
        )
        .env(CALLER_WORKTREE, &wt)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let (started_sender, started_group) = std::sync::mpsc::channel();
    let (ended_sender, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Each open of a FIFO waits for the other side's: the command opens
        // `alive` first, then writes its group to `started`.
        let mut alive = std::fs::File::open(&alive).unwrap();
        let group: i32 = std::fs::read_to_string(&started)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        started_sender.send(group).unwrap();
        // The end of `alive` comes once no process holds it open for
        // writing: once the command and every process it started are gone.
        std::io::copy(&mut alive, &mut std::io::sink()).unwrap();
        let _ = ended_sender.send(());
    });
    let Ok(group) = started_group.recv_timeout(Duration::from_secs(60)) else {
        let _ = caller.kill();
        panic!("the command never started");
    };
    // SIGKILL: the caller gets no chance to stop anything.
    caller.kill().unwrap();
    caller.wait().unwrap();
    let stopped = ended.recv_timeout(Duration::from_secs(10)).is_ok();
    if !stopped && crate::procs::runs(-group) {
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    assert!(
        stopped,
        "the command's process group {group} outlived its caller"
    );
}

/// Commits a post-checkout hook to the source repo and refreshes the host's
/// clone, as a repo with `scripts/hooks/post-checkout` has.
fn add_hook(s: &Setup, body: &str) {
    let hooks = s.source.join("scripts/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    std::fs::write(&hook, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    git(&s.source, &["add", "scripts/hooks/post-checkout"]);
    git(&s.source, &["commit", "-q", "-m", "Add the hook"]);
    git(&s.home.join("dev/demo"), &["fetch", "-q", "origin"]);
}

#[test]
fn a_new_remote_worktree_runs_the_repos_post_checkout_with_the_hosts_root() {
    let s = setup("remote-hook");
    add_hook(&s, r#"echo "$BUILDS_ROOT $1 $3 $(pwd)" > ../hook.txt"#);
    let places = s.scratch.places();
    let job = request(
        What::Ref("main".to_string()),
        &["sh", "-c", "echo \"$BUILDS_ROOT\" > ../run-root.txt"],
    );
    assert_eq!(
        run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap(),
        0
    );
    let wts = s.home.join("dev/demo-worktrees");
    let hook = std::fs::read_to_string(wts.join("hook.txt")).unwrap();
    let root = format!("{}/builds-root.noindex", s.home.display());
    assert_eq!(
        hook.trim(),
        format!(
            "{root} 0000000000000000000000000000000000000000 1 {}/remote-demo",
            wts.display()
        )
    );
    assert_eq!(
        std::fs::read_to_string(wts.join("run-root.txt"))
            .unwrap()
            .trim(),
        root,
        "the command sees the same root"
    );
}

#[test]
fn a_failing_post_checkout_fails_the_job_and_releases_the_claim() {
    let s = setup("remote-hook-fails");
    add_hook(&s, "echo broken >&2; exit 4");
    let places = s.scratch.places();
    let job = request(What::Ref("main".to_string()), &["true"]);
    let Err(Fail::Message(message)) = run_job(&places, &s.ssh, &s.host, &s.source, &job) else {
        panic!("expected a failure");
    };
    assert!(message.contains("post-checkout failed"), "{message}");
    assert!(message.contains("broken"), "{message}");
    assert!(!s.home.join("dev/demo-worktrees/remote-demo.job").exists());
    assert_eq!(refs(&s.home.join("dev/demo")), "");
}

#[test]
fn every_job_runs_the_hosts_builds_setup_in_its_worktree_before_the_hook() {
    let s = setup("remote-setup");
    add_hook(&s, r#"echo "hook $(pwd)" >> "$HOME/builds-runs.txt""#);
    let places = s.scratch.places();
    let job = request(
        What::Ref("main".to_string()),
        &[
            "sh",
            "-c",
            "echo \"command $(pwd)\" >> \"$HOME/builds-runs.txt\"",
        ],
    );
    for _ in 0..2 {
        assert_eq!(
            run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap(),
            0
        );
    }
    let wt = s.home.join("dev/demo-worktrees/remote-demo");
    let wt = wt.display();
    // `builds setup` links the build folders again in the worktree the
    // earlier job left, since a repo without a post-checkout has nothing
    // else that links them.
    let one = format!("builds setup {wt}\nhook {wt}\ncommand {wt}\n");
    assert_eq!(
        std::fs::read_to_string(s.home.join("builds-runs.txt")).unwrap(),
        one.repeat(2)
    );
}

#[test]
fn a_hook_checks_job_sets_its_worktree_up_without_preparing_it() {
    let s = setup("remote-setup-hook");
    let places = s.scratch.places();
    let mut job = request(What::Ref("main".to_string()), &["true"]);
    job.prepare = crate::setup::Prepare::No;
    assert_eq!(
        run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap(),
        0
    );
    let wt = s.home.join("dev/demo-worktrees/remote-demo");
    assert_eq!(
        std::fs::read_to_string(s.home.join("builds-runs.txt")).unwrap(),
        format!("builds setup --no-prepare {}\n", wt.display())
    );
}

#[test]
fn a_failing_builds_setup_fails_the_job_and_releases_the_claim() {
    let s = setup("remote-setup-fails");
    host_builds(&s.home, "echo 'build volume gone' >&2; exit 4");
    let places = s.scratch.places();
    let job = request(What::Ref("main".to_string()), &["true"]);
    let Err(Fail::Message(message)) = run_job(&places, &s.ssh, &s.host, &s.source, &job) else {
        panic!("expected a failure");
    };
    assert!(message.contains("builds setup failed"), "{message}");
    assert!(message.contains("build volume gone"), "{message}");
    assert!(!s.home.join("dev/demo-worktrees/remote-demo.job").exists());
    assert_eq!(refs(&s.home.join("dev/demo")), "");
}

#[test]
fn a_host_with_a_build_volume_uses_it_as_its_root() {
    let s = setup("remote-volume-root");
    std::fs::create_dir_all(s.home.join("volume")).unwrap();
    let job = request(
        What::Ref("main".to_string()),
        &["sh", "-c", "echo \"$BUILDS_ROOT\" > ../run-root.txt"],
    );
    let places = s.scratch.places();
    assert_eq!(
        run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap(),
        0
    );
    let seen = std::fs::read_to_string(s.home.join("dev/demo-worktrees/run-root.txt")).unwrap();
    assert_eq!(seen.trim(), format!("{}/volume", s.home.display()));
}

#[test]
fn a_remote_step_runs_with_the_hosts_root_and_path() {
    let s = setup("remote-step-env");
    let places = s.scratch.places();
    let wt = s.home.join("dev/demo-worktrees/app-1");
    std::fs::create_dir_all(wt.with_extension("job")).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    // The claim folder that holds the step's output FIFOs sits beside the
    // worktree.
    std::fs::create_dir_all(format!("{}.job", wt.display())).unwrap();
    let transport = Transport {
        places: &places,
        ssh: &s.ssh,
        host: &s.host,
    };
    let command = args(&[
        "sh",
        "-c",
        "echo \"$BUILDS_ROOT\" > env.txt; echo \"$PATH\" >> env.txt; echo \"$TMPDIR\" >> env.txt",
    ]);
    let ran = run_command(&transport, &wt.display().to_string(), &command, None).unwrap();
    assert_eq!(ran, (0, None));
    // A cargo command's messages pass through builds, which times the
    // waits for cargo's file locks there.
    let cargo = args(&[
        "sh",
        "-c",
        "echo '    Blocking waiting for file lock on build directory' >&2; sleep 0.3; echo '   Compiling demo' >&2",
    ]);
    let (code, waited) = run_command(
        &transport,
        &wt.display().to_string(),
        &cargo,
        Some(&crate::cargo_run_outcome::Watch::default()),
    )
    .unwrap();
    assert_eq!(code, 0);
    let waited = waited.expect("run_command reads cargo's messages");
    assert!(waited >= Duration::from_millis(300), "{waited:?}");
    let seen = std::fs::read_to_string(wt.join("env.txt")).unwrap();
    let mut lines = seen.lines();
    assert_eq!(
        lines.next().unwrap(),
        format!("{}/builds-root.noindex", s.home.display())
    );
    let path = lines.next().unwrap();
    assert!(
        path.starts_with(&format!(
            "{home}/.cargo/bin:{home}/.local/bin:",
            home = s.home.display()
        )),
        "{path}"
    );
    // The command's TMPDIR is a folder of the run on the host's build root,
    // which the end of the command deleted.
    let tmpdir = std::path::PathBuf::from(lines.next().unwrap());
    assert_eq!(
        tmpdir.parent().unwrap(),
        s.home
            .join("builds-root.noindex")
            .join(crate::run_tmp::FOLDER)
    );
    assert!(!tmpdir.exists());
}

/// The sweep lists the host's run folders, judges each name by this Mac's
/// process table, and deletes on the host the folders of runs that ended.
#[test]
fn a_sweep_deletes_on_the_host_the_run_folders_of_ended_runs() {
    let s = setup("sweep-host");
    let places = s.scratch.places();
    let parent = s
        .home
        .join("builds-root.noindex")
        .join(crate::run_tmp::FOLDER);
    let mut ended = std::process::Command::new("true").spawn().unwrap();
    let gone = format!("{}-1", ended.id());
    ended.wait().unwrap();
    let running = crate::remote::job_id();
    for name in [gone.as_str(), running.as_str(), "not-a-run"] {
        std::fs::create_dir_all(parent.join(name).join("inside")).unwrap();
    }
    let swept = crate::run_tmp::sweep(
        &places,
        &s.ssh,
        &s.host,
        &crate::job_claim::Holders::read().unwrap(),
    )
    .unwrap();
    assert_eq!(swept.deleted, [format!(".tmp/{gone}")]);
    assert_eq!(swept.unexpected, ["not-a-run"]);
    assert!(!parent.join(&gone).exists());
    assert!(parent.join(&running).is_dir() && parent.join("not-a-run").is_dir());
}

#[test]
fn sends_the_whole_working_tree_without_moving_a_ref_or_the_index() {
    let s = setup("remote-working");
    let places = s.scratch.places();
    // A tracked file the person edits, one they delete, a staged new file, an
    // untracked file, and an ignored one.
    std::fs::write(s.source.join(".gitignore"), "ignored.txt\n").unwrap();
    git(&s.source, &["add", ".gitignore"]);
    git(&s.source, &["commit", "-q", "-m", "ignore"]);
    std::fs::write(s.source.join("tracked.txt"), "one\n").unwrap();
    git(&s.source, &["add", "tracked.txt"]);
    git(&s.source, &["commit", "-q", "-m", "track"]);
    std::fs::write(s.source.join("tracked.txt"), "edited\n").unwrap();
    std::fs::write(s.source.join("staged.txt"), "staged\n").unwrap();
    git(&s.source, &["add", "staged.txt"]);
    std::fs::write(s.source.join("untracked.txt"), "untracked\n").unwrap();
    std::fs::write(s.source.join("ignored.txt"), "ignored\n").unwrap();
    let status_before = git_text(
        boards::git(&s.source).args(["status", "--porcelain"]),
        "status",
    )
    .unwrap();
    let head_before = commit_of(&s.source, &What::Ref("HEAD".to_string())).unwrap();
    let job = request(
        What::Working,
        &[
            "sh",
            "-c",
            "cat tracked.txt staged.txt untracked.txt > ../out.txt; ls ignored.txt 2>/dev/null > ../ignored.txt || true",
        ],
    );
    assert_eq!(
        run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap(),
        0
    );
    let wts = s.home.join("dev/demo-worktrees");
    assert_eq!(
        std::fs::read_to_string(wts.join("out.txt")).unwrap(),
        "edited\nstaged\nuntracked\n"
    );
    assert_eq!(
        std::fs::read_to_string(wts.join("ignored.txt")).unwrap(),
        "",
        "an ignored file stays"
    );
    // A deleted tracked file is gone in the sent tree.
    std::fs::remove_file(s.source.join("tracked.txt")).unwrap();
    let job = request(
        What::Working,
        &[
            "sh",
            "-c",
            "ls tracked.txt 2>/dev/null > ../deleted.txt || true",
        ],
    );
    run_job(&places, &s.ssh, &s.host, &s.source, &job).unwrap();
    assert_eq!(
        std::fs::read_to_string(wts.join("deleted.txt")).unwrap(),
        ""
    );
    std::fs::write(s.source.join("tracked.txt"), "edited\n").unwrap();
    assert_eq!(
        git_text(
            boards::git(&s.source).args(["status", "--porcelain"]),
            "status",
        )
        .unwrap(),
        status_before,
        "the index and the files are as they were"
    );
    assert_eq!(
        commit_of(&s.source, &What::Ref("HEAD".to_string())).unwrap(),
        head_before
    );
    assert_eq!(refs(&s.source), "");
    assert_eq!(refs(&s.home.join("dev/demo")), "");
    let leftovers: Vec<_> = std::fs::read_dir(s.source.join(".git"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("builds-job-index")
        })
        .collect();
    assert!(leftovers.is_empty(), "the scratch index is deleted");
}

#[test]
fn a_command_on_this_mac_runs_without_the_git_variables_of_a_hook() {
    let s = setup("remote-git-env");
    let places = s.scratch.places();
    let ssh = &s.ssh;
    let host = Host::local();
    let transport = Transport {
        places: &places,
        ssh,
        host: &host,
    };
    let command = transport.command("true").unwrap();
    let removed: Vec<String> = command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for name in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        assert!(
            removed.iter().any(|r| r == name),
            "{name} not removed: {removed:?}"
        );
    }
}

/// An ssh that, like the real one, returns only once nothing on the host
/// holds the command's output open, with the command's exit status: it
/// passes the output of `fake_ssh` through a pipe, which a process the
/// command left behind would keep open.
fn holding_ssh(dir: &Path, fake: &Ssh) -> Ssh {
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("ssh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/bash\nset -o pipefail\n{} \"$@\" | cat\n",
            crate::host::sh_quote(
                &fake
                    .system()
                    .path(crate::system::Program::Ssh)
                    .unwrap()
                    .to_string_lossy()
            )
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    crate::testing::ssh_at(&program)
}

/// An app checkout on the host, with the claim folder that holds a step's
/// output FIFOs beside it.
fn app_checkout(s: &Setup) -> PathBuf {
    let wt = s.home.join("dev/demo-worktrees/app-1");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::create_dir_all(format!("{}.job", wt.display())).unwrap();
    wt
}

#[test]
fn a_command_that_leaves_a_process_holding_its_output_ends_with_its_status() {
    let s = setup("remote-leftover");
    let places = s.scratch.places();
    let ssh = holding_ssh(&s.scratch.dir.join("holding-bin"), &s.ssh);
    let wt = app_checkout(&s);
    let transport = Transport {
        places: &places,
        ssh: &ssh,
        host: &s.host,
    };
    // The command exits 3 and leaves a process in a process group of its
    // own that holds the command's output open, as a build daemon does.
    let command = args(&[
        "sh",
        "-c",
        "set -m; sleep 60 & echo $! > left.pid; printf 'last words'; printf 'last error' >&2; exit 3",
    ]);
    let started = std::time::Instant::now();
    let ran = run_command(
        &transport,
        &wt.display().to_string(),
        &command,
        Some(&crate::cargo_run_outcome::Watch::default()),
    );
    let took = started.elapsed();
    let left = std::fs::read_to_string(wt.join("left.pid")).unwrap();
    unsafe { libc::kill(left.trim().parse().unwrap(), libc::SIGKILL) };
    let (code, _) = ran.unwrap();
    assert_eq!(code, 3);
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn a_command_passes_on_all_it_printed() {
    let s = setup("remote-relay");
    let places = s.scratch.places();
    let wt = app_checkout(&s);
    let transport = Transport {
        places: &places,
        ssh: &s.ssh,
        host: &s.host,
    };
    let output = s.scratch.dir.join("output");
    // The script's own output goes to a file, so this test reads what the
    // relays passed on.
    let mut start = transport
        .command(&run_script(
            &s.host,
            &wt.display().to_string(),
            &args(&["sh", "-c", "seq 1 20000; printf 'no end of line'; exit 4"]),
            "test",
        ))
        .unwrap();
    let mut started = start
        .stdin(Stdio::piped())
        .stdout(std::fs::File::create(&output).unwrap())
        .spawn()
        .unwrap();
    // The test holds the script's stdin open while the command runs, as
    // `run_command` does.
    let lifeline = started.stdin.take();
    let status = started.wait().unwrap();
    drop(lifeline);
    assert_eq!(status.code(), Some(4));
    let text = std::fs::read_to_string(&output).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 20001, "{}", &text[text.len() - 100..]);
    assert_eq!(lines[19999], "20000");
    assert_eq!(lines[20000], "no end of line");
    assert!(!wt.with_extension("job").join("out").exists());
}

#[test]
fn a_finished_job_leaves_no_job_status_lines_on_stderr() {
    let s = setup("remote-job-status");
    let places = s.scratch.places();
    let wt = app_checkout(&s);
    let transport = Transport {
        places: &places,
        ssh: &s.ssh,
        host: &s.host,
    };
    let errors = s.scratch.dir.join("errors");
    let mut start = transport
        .command(&run_script(
            &s.host,
            &wt.display().to_string(),
            &args(&["sh", "-c", "echo out; echo oops >&2"]),
            "test",
        ))
        .unwrap();
    let mut started = start
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&errors).unwrap())
        .spawn()
        .unwrap();
    let lifeline = started.stdin.take();
    let status = started.wait().unwrap();
    drop(lifeline);
    assert_eq!(status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&errors).unwrap(), "oops\n");
}

/// Only one process at a time opens windows on a Mac, and builds cannot tell
/// whether a command opens any: a job on a Mac holds the host's window lock
/// while its command runs, naming itself as the holder, and lets it go when
/// the job ends. A job on a Linux host, which has no display, takes none.
#[test]
fn a_job_on_a_mac_holds_the_hosts_window_lock_while_it_runs() {
    let s = setup("remote-window");
    let places = s.scratch.places();
    let holder = places.app_window_lock_file("m1").with_file_name("m1.run");
    let copy = format!("cat {} > ../holder.txt 2>&1", holder.display());
    let job = request(What::Ref("main".to_string()), &["sh", "-c", &copy]);
    let wts = s.home.join("dev/demo-worktrees");

    let code = run_job_on(&places, &s.ssh, &s.host, Os::Mac, &s.source, &job).unwrap();
    assert_eq!(code, 0);
    let held = std::fs::read_to_string(wts.join("holder.txt")).unwrap();
    assert!(
        held.contains(&format!("demo's job `sh -c {copy}`")),
        "{held}"
    );
    assert!(!holder.exists(), "the job let the lock go");

    std::fs::remove_file(wts.join("holder.txt")).unwrap();
    let code = run_job_on(&places, &s.ssh, &s.host, Os::Linux, &s.source, &job).unwrap();
    assert_eq!(code, 1, "cat finds no holder");
    let held = std::fs::read_to_string(wts.join("holder.txt")).unwrap();
    assert!(held.contains("No such file"), "{held}");
}

fn sh(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    command
}

/// Runs `pipe_within` on another thread, so a copy that never ends fails the
/// test instead of hanging it.
fn piped(sender: Command, receiver: Command, stall: Duration) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(pipe_within(sender, receiver, "the test files", stall));
    });
    rx.recv_timeout(Duration::from_secs(20))
        .expect("the copy never ended")
}

#[test]
fn a_copy_whose_receiving_end_dies_fails_and_stops_the_sending_end() {
    let error = piped(
        sh("yes"),
        sh("echo gone >&2; exit 3"),
        Duration::from_secs(60),
    )
    .unwrap_err()
    .text();
    assert!(error.contains("receiving the test files failed"), "{error}");
    assert!(error.contains("gone"), "{error}");
}

#[test]
fn a_copy_that_stops_moving_fails_naming_what_it_copied() {
    let error = piped(
        sh("echo start; echo ssh-said-why >&2; sleep 60"),
        sh("cat > /dev/null"),
        Duration::from_secs(1),
    )
    .unwrap_err()
    .text();
    assert!(
        error.contains("copying the test files stopped moving for 1s"),
        "{error}"
    );
    assert!(error.contains("ssh-said-why"), "{error}");
}

#[test]
fn a_copy_that_moves_and_ends_succeeds_and_a_failing_sender_fails() {
    piped(
        sh("head -c 3000000 /dev/zero"),
        sh("wc -c > /dev/null"),
        Duration::from_secs(60),
    )
    .unwrap();
    let error = piped(
        sh("echo no >&2; exit 2"),
        sh("cat > /dev/null"),
        Duration::from_secs(60),
    )
    .unwrap_err()
    .text();
    assert!(error.contains("copying the test files failed"), "{error}");
    assert!(error.contains("no"), "{error}");
}

/// `tar -x` ends once it reads the archive's end marker, before it sees the
/// sender's end of file, and the sender may still have padding to write.
#[test]
fn a_receiver_that_ends_successfully_before_the_sender_does_not_fail_the_copy() {
    piped(
        sh("head -c 3000000 /dev/zero"),
        sh("head -c 10 > /dev/null"),
        Duration::from_secs(60),
    )
    .unwrap();
}

#[test]
fn a_copy_that_keeps_moving_bytes_past_the_stall_limit_succeeds() {
    piped(
        sh("for i in 1 2 3 4 5 6 7 8; do echo x; sleep 0.4; done"),
        sh("cat > /dev/null"),
        Duration::from_secs(1),
    )
    .unwrap();
}

#[test]
fn a_stalled_copy_whose_receiver_already_ended_still_fails_as_stalled() {
    let error = piped(sh("sleep 60"), sh("exit 0"), Duration::from_secs(1))
        .unwrap_err()
        .text();
    assert!(error.contains("stopped moving"), "{error}");
}

/// A group whose last process ends between the run script's look at it and
/// its SIGTERM is stopped: the job ends with its command's status.
#[test]
fn a_group_that_ends_before_its_sigterm_fails_no_job() {
    let scratch = Scratch::new("remote-gone-group");
    let worktree = scratch.dir.join("worktree");
    std::fs::create_dir_all(worktree.with_extension("job")).unwrap();
    let local = crate::host::Host::local();
    let script = run_script(
        &local,
        &worktree.display().to_string(),
        &["true".to_string()],
        "77-1795067690799282000",
    );
    std::fs::create_dir_all(&worktree).unwrap();
    // `true` leaves nothing in its group. Until the script has sent the
    // command's group a SIGTERM, this kill says the group has a process, as
    // a look at it does just before the last one ends.
    let termed = scratch.dir.join("termed");
    let kill = format!(
        r#"kill() {{
  if [ "$1" = -0 ] && [ ! -e '{termed}' ]; then return 0; fi
  if [ "$1" = -TERM ] && [ "$2" = "-$pid" ]; then : > '{termed}'; fi
  command kill "$@"
}}
"#,
        termed = termed.display()
    );
    let output = scratch
        .system()
        .sh(&format!("{kill}{script}"))
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        termed.exists(),
        "the script sent the group no SIGTERM: {said}"
    );
    assert_eq!(output.status.code(), Some(0), "{said}");
}
