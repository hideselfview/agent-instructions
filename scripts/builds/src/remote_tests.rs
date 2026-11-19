use super::*;
use crate::host::tests::fake_ssh;
use crate::testing::{Scratch, git, gone_pid, host_builds};

/// Runs a job that nothing stops.
fn run_job(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    source: &Path,
    request: &Request,
) -> Result<i32> {
    super::run_job(places, ssh, host, source, request, &AtomicBool::new(false))
}

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
    let ssh = fake_ssh(&scratch.dir.join("bin"), &home);
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
        prepare: true,
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
            prepare: true,
        }
    );
    let staged = parse(&args(&["run", "bae", "--staged", "--", "true"])).unwrap();
    assert_eq!(staged.what, What::Staged);
    let working = parse(&args(&["run", "bae", "--working", "--", "true"])).unwrap();
    assert_eq!(working.what, What::Working);
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
    std::fs::write(claim.join("pid"), "73910\n").unwrap();

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
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stopper = stop.clone();
    let waiting = marker.clone();
    let signaller = std::thread::spawn(move || {
        while !waiting.exists() {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Give run_command a moment to record the pid, then stop.
        std::thread::sleep(Duration::from_millis(300));
        stopper.store(true, Ordering::SeqCst);
    });
    let code = super::run_job(&places, &s.ssh, &s.host, &s.source, &job, &stop).unwrap();
    signaller.join().unwrap();
    assert_eq!(code, STOPPED_EXIT);
    assert!(!s.home.join("dev/demo-worktrees/remote-demo.job").exists());
    assert_eq!(refs(&s.home.join("dev/demo")), "");
}

/// A process in a process group of its own, as an earlier step's command
/// was, whose pid an app checkout's claim folder still names.
fn earlier_step() -> std::process::Child {
    use std::os::unix::process::CommandExt;
    Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap()
}

#[test]
fn a_stop_signals_only_the_process_group_of_the_run_it_stops() {
    let s = setup("remote-stop-run");
    let places = s.scratch.places();
    let wt = app_checkout(&s);
    let transport = Transport {
        places: &places,
        ssh: &s.ssh,
        host: &s.host,
    };
    let mut earlier = earlier_step();
    std::fs::write(
        format!("{}.job/pid", wt.display()),
        format!("{} earlier-run\n", earlier.id()),
    )
    .unwrap();
    let wt = wt.display().to_string();
    transport
        .capture(&stop_script(&wt, "this-run"), "stopping the command")
        .unwrap();
    assert!(
        earlier.try_wait().unwrap().is_none(),
        "a stop of another run signals nothing"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{wt}.job/stop")).unwrap(),
        "this-run\n",
        "the stop records its run for the run script to read"
    );
    transport
        .capture(&stop_script(&wt, "earlier-run"), "stopping the command")
        .unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(earlier.wait().unwrap().signal(), Some(libc::SIGTERM));
}

#[test]
fn a_stop_before_the_step_writes_its_pid_waits_for_it_and_spares_the_earlier_steps_pid() {
    let s = setup("remote-stop-early");
    let places = s.scratch.places();
    let wt = app_checkout(&s);
    let transport = Transport {
        places: &places,
        ssh: &s.ssh,
        host: &s.host,
    };
    let mut earlier = earlier_step();
    std::fs::write(
        format!("{}.job/pid", wt.display()),
        format!("{} earlier-run\n", earlier.id()),
    )
    .unwrap();
    let started = std::time::Instant::now();
    // The stop comes before the step starts.
    let ran = run_command(
        &transport,
        &AtomicBool::new(true),
        &wt.display().to_string(),
        &args(&["sleep", "10"]),
        false,
    );
    let took = started.elapsed();
    let spared = earlier.try_wait().unwrap().is_none();
    let _ = earlier.kill();
    let _ = earlier.wait();
    assert_eq!(ran.unwrap(), (STOPPED_EXIT, None));
    assert!(spared, "the stop signals only this run's command");
    assert!(
        took < Duration::from_secs(5),
        "the stop stops the step: {took:?}"
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
    job.prepare = false;
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
    // The claim folder the step records its pid in sits beside the worktree.
    std::fs::create_dir_all(format!("{}.job", wt.display())).unwrap();
    let transport = Transport {
        places: &places,
        ssh: &s.ssh,
        host: &s.host,
    };
    let command = args(&[
        "sh",
        "-c",
        "echo \"$BUILDS_ROOT\" > env.txt; echo \"$PATH\" >> env.txt",
    ]);
    let ran = run_command(
        &transport,
        &AtomicBool::new(false),
        &wt.display().to_string(),
        &command,
        false,
    )
    .unwrap();
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
        &AtomicBool::new(false),
        &wt.display().to_string(),
        &cargo,
        true,
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
    let command = transport.command("true");
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
            crate::host::sh_quote(&fake.program)
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    Ssh {
        program: program.display().to_string(),
    }
}

/// An app checkout on the host, with the claim folder a step records its pid
/// in beside it.
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
        &AtomicBool::new(false),
        &wt.display().to_string(),
        &command,
        true,
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
    let mut start = transport.command(&run_script(
        &s.host,
        &wt.display().to_string(),
        &args(&["sh", "-c", "seq 1 20000; printf 'no end of line'; exit 4"]),
        "test",
    ));
    let status = start
        .stdout(std::fs::File::create(&output).unwrap())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(4));
    let text = std::fs::read_to_string(&output).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 20001, "{}", &text[text.len() - 100..]);
    assert_eq!(lines[19999], "20000");
    assert_eq!(lines[20000], "no end of line");
    assert!(!wt.with_extension("job").join("out").exists());
}
