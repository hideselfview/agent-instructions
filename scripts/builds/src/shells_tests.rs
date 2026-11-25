use super::*;

const NOW: i64 = 1_800_000_000;
const HOME: &str = "/Users/x";

fn places() -> Vec<Place> {
    ["demo/side-1", "demo/side-2"]
        .iter()
        .map(|label| {
            let name = label.rsplit('/').next().unwrap();
            Place {
                label: label.to_string(),
                repo: "demo".to_string(),
                top: PathBuf::from(format!("{HOME}/dev/demo-worktrees/{name}")),
                root: PathBuf::from(format!("/volume/demo/{name}")),
            }
        })
        .collect()
}

fn side(n: usize) -> PathBuf {
    PathBuf::from(format!("{HOME}/dev/demo-worktrees/side-{n}"))
}

/// An agent shell as Claude Code starts it, <minutes> old, in <cwd>.
fn shell(pid: i32, minutes: i64, cwd: &Path, command: &str) -> Entry {
    let script = format!(
        "source {HOME}/.claude/shell-snapshots/snapshot-zsh-1.sh 2>/dev/null || true && setopt NO_EXTENDED_GLOB 2>/dev/null || true && eval '{}' < /dev/null && pwd -P >| /tmp/claude-1-cwd",
        command.replace('\'', "'\"'\"'")
    );
    Entry {
        pid,
        ppid: 0,
        started: NOW - minutes * 60,
        cpu_seconds: 0.01,
        argv: vec!["/bin/zsh".to_string(), "-c".to_string(), script],
        cwd: Some(cwd.to_path_buf()),
        agent: None,
    }
}

/// A process under <parent>, using <cpu> seconds of CPU.
fn child(pid: i32, parent: &Entry, cpu: f64, argv: &[&str]) -> Entry {
    Entry {
        pid,
        ppid: parent.pid,
        started: parent.started,
        cpu_seconds: cpu,
        argv: argv.iter().map(|a| a.to_string()).collect(),
        cwd: parent.cwd.clone(),
        agent: None,
    }
}

fn judge_all(entries: &[Entry]) -> Shells {
    judge(
        entries,
        NOW,
        Path::new(HOME),
        &places(),
        &HashSet::new(),
        &HashSet::new(),
    )
}

fn reasons(shells: &Shells) -> Vec<(i32, Reason)> {
    shells
        .junk
        .iter()
        .map(|j| (j.pid, j.reason.clone()))
        .collect()
}

fn waits(by: &str) -> Reason {
    Reason::Waits(by.to_string())
}

#[test]
fn a_shell_shows_its_command_without_the_tools_wrapper() {
    let entry = shell(10, 1, &side(1), "echo 'it'\"'\"'s'");
    assert_eq!(command_of(&entry.argv), "echo 'it'\"'\"'s'");
    let (dir, rest) = strip_cd("cd ~/dev/demo-worktrees/side-2 && cargo test -p a");
    assert_eq!(dir.as_deref(), Some("~/dev/demo-worktrees/side-2"));
    assert_eq!(rest, "cargo test -p a");
    assert!(runs_the_dashboard(
        "/Users/x/.local/bin/builds dashboard --port 7787"
    ));
    assert!(!runs_the_dashboard("builds status"));
}

#[test]
fn commands_split_like_a_shell_reads_them() {
    let words =
        |text: &str| -> Vec<String> { commands(text).iter().map(|c| c.join(" ")).collect() };
    assert_eq!(
        words("until ! pgrep -f 'cargo test'; do sleep 5; done"),
        ["until ! pgrep -f cargo test", "sleep 5", "done"]
    );
    assert_eq!(
        words("for i in $(seq 1 3); do xcrun swiftc -parse f; done"),
        ["for i in $", "seq 1 3", "xcrun swiftc -parse f", "done"]
    );
    assert_eq!(
        words("git commit -m \"wait; sleep until while\""),
        ["git commit -m wait; sleep until while"]
    );
    assert_eq!(
        words("cat <<'EOF'\nsleep 9\nEOF\necho done"),
        ["cat", "echo done"]
    );
    assert_eq!(words("FOO=1 nohup sleep 3 # sleep 4"), ["sleep 3"]);
}

#[test]
fn a_wait_is_junk_whatever_form_it_takes() {
    let a = shell(
        1,
        3,
        &side(1),
        "until ! pgrep -f 'cargo test'; do sleep 5; done",
    );
    let b = shell(2, 3, &side(1), "cd /tmp && sleep 600");
    let b_child = child(21, &b, 0.0, &["sleep", "600"]);
    let c = shell(
        3,
        3,
        &side(1),
        "python3 -c \"import time; time.sleep(598)\"",
    );
    let c_child = child(
        31,
        &c,
        0.0,
        &["python3", "-c", "import time; time.sleep(598)"],
    );
    let d = shell(4, 3, &side(1), "caffeinate -i -t 119");
    let d_child = child(41, &d, 0.0, &["caffeinate", "-i", "-t", "119"]);
    let e = shell(5, 3, &side(1), "caffeinate -w $(pgrep -f 'cargo test')");
    let e_child = child(51, &e, 0.0, &["caffeinate", "-w", "4242"]);
    let f = shell(6, 3, &side(1), "lsof -p 4242 +r 5");
    let f_child = child(61, &f, 0.0, &["lsof", "-p", "4242", "+r", "5"]);
    let g = shell(
        7,
        3,
        &side(1),
        "for i in $(seq 1 5000); do xcrun swiftc -parse f.swift; done",
    );
    let h = shell(8, 3, &side(1), "scripts/run-tests.sh");
    let h_child = child(81, &h, 0.0, &["/bin/sleep", "30"]);
    let found = judge_all(&[
        a, b, b_child, c, c_child, d, d_child, e, e_child, f, f_child, g, h, h_child,
    ]);
    assert_eq!(
        reasons(&found),
        vec![
            (1, waits("an until loop")),
            (2, waits("sleep")),
            (3, waits("time.sleep")),
            (4, waits("caffeinate -t/-w")),
            (5, waits("caffeinate -t/-w")),
            (6, waits("lsof +r")),
            (7, waits("a for loop over seq")),
            (8, waits("sleep")),
        ]
    );
    assert_eq!(found.running, 8);
}

#[test]
fn a_sleep_that_already_finished_is_not_a_wait() {
    // pid 8015 on 2026-11-14: the `sleep 1` ended and `builds app-build` ran.
    let command = "kill 99908; sleep 1; cd ~/dev/demo-worktrees/side-2 && builds app-build demo reads-import-count macos macos-tests --test mac PaginatedListTests 2>&1 | tail -60";
    let build = shell(8015, 5, &side(1), command);
    let app_build = child(
        8206,
        &build,
        0.1,
        &[
            "builds",
            "app-build",
            "demo",
            "reads-import-count",
            "macos",
            "macos-tests",
            "--test",
            "mac",
            "PaginatedListTests",
        ],
    );
    let tail = child(8207, &build, 0.0, &["tail", "-60"]);
    let found = judge_all(&[build, app_build, tail]);
    assert_eq!(found.running, 1);
    assert!(found.junk.is_empty(), "{:?}", reasons(&found));
}

#[test]
fn caffeinate_waits_only_on_its_own_options_with_no_program() {
    let argv = |words: &[&str]| -> Vec<String> { words.iter().map(|w| w.to_string()).collect() };
    let waits_as_caffeinate = |words: &[&str]| waits_as(&argv(words)).is_some();
    assert!(waits_as_caffeinate(&["caffeinate", "-w4242"]));
    assert!(waits_as_caffeinate(&["caffeinate", "-it", "119"]));
    assert!(waits_as_caffeinate(&["caffeinate", "-t", "119", "-i"]));
    assert!(waits_as_caffeinate(&[
        "/usr/bin/caffeinate",
        "-w",
        "4242",
        "--"
    ]));
    // tar's -tf is not caffeinate's -t.
    assert!(!waits_as_caffeinate(&[
        "caffeinate",
        "-i",
        "tar",
        "-tf",
        "a.tar"
    ]));
    // With a program caffeinate ignores -t and -w and runs the program.
    assert!(!waits_as_caffeinate(&[
        "caffeinate",
        "-w",
        "4242",
        "tar",
        "-xf",
        "a.tar"
    ]));
    assert!(!waits_as_caffeinate(&[
        "caffeinate",
        "-t",
        "60",
        "--",
        "make"
    ]));
    assert!(!waits_as_caffeinate(&["caffeinate", "-i"]));
}

#[test]
fn a_wait_loop_around_a_running_build_is_not_junk() {
    let looped = shell(1, 3, &side(1), "until cargo test -p a; do sleep 5; done");
    let cargo = child(11, &looped, 5.0, &["cargo", "test", "-p", "a"]);
    let polling = shell(
        2,
        3,
        &side(1),
        "while pgrep -f xcodebuild; do sleep 5; done",
    );
    let pgrep = child(21, &polling, 0.0, &["pgrep", "-f", "xcodebuild"]);
    let found = judge_all(&[looped, cargo, polling, pgrep]);
    assert_eq!(reasons(&found), vec![(2, waits("a while loop"))]);
}

#[test]
fn a_junk_shell_names_the_process_that_started_it() {
    let mut polling = shell(7, 3, &side(1), "caffeinate -w 4242");
    polling.ppid = 555;
    let live = child(71, &polling, 0.0, &["caffeinate", "-w", "4242"]);
    let found = judge_all(&[polling, live]);
    assert_eq!(found.junk[0].parent, 555);
    assert!(
        lines(&found)[1]
            .starts_with("    pid 7 (agent unknown, started by pid 555) 3m  demo/side-1  waits"),
        "{}",
        lines(&found)[1]
    );
}

#[test]
fn stopping_a_shell_takes_its_tree_deepest_first() {
    let pairs = [(1, 0), (10, 1), (11, 10), (12, 10), (20, 2), (13, 12)];
    assert_eq!(tree_of(&pairs, 10), vec![13, 12, 11, 10]);
    let junk = Junk {
        pid: 10,
        parent: 1,
        agent: Some("a1".to_string()),
        minutes: 4,
        worktree: "demo/side-1".to_string(),
        reason: waits("sleep"),
        command: "sleep 600".to_string(),
    };
    assert_eq!(
        stopped_line(&junk, 4),
        "stopped pid 10 and 3 processes under it (agent a1, started by pid 1, demo/side-1): waits (sleep): sleep 600"
    );
}

#[test]
fn text_that_only_mentions_a_wait_is_not_one() {
    let commit = shell(
        1,
        3,
        &side(1),
        "git commit -m 'wait until it is done, sleep well' && cat <<'EOF'\nwhile true; do sleep 1; done\nEOF",
    );
    let build = shell(2, 3, &side(1), "caffeinate -i cargo build");
    let reader = shell(
        3,
        3,
        &side(1),
        "while read line; do echo $line; done < list",
    );
    let grep = shell(4, 3, &side(1), "git log --grep sleep");
    assert!(judge_all(&[commit, build, reader, grep]).junk.is_empty());
}

#[test]
fn a_search_over_the_whole_disk_is_junk() {
    let a = shell(1, 3, &side(1), "find / -name '*.swift' -mtime -1");
    let b = shell(2, 3, &side(1), "du -sh ~");
    let c = shell(3, 3, &side(1), "cd /tmp && find /Users/x -name a");
    let d = shell(4, 3, &side(1), "find . -name '*.rs'");
    let e = shell(
        5,
        3,
        &side(1),
        "du -sh /Users/x/dev/demo-worktrees/side-1/target",
    );
    let f = shell(6, 3, &side(1), "scripts/search.sh");
    let f_child = child(61, &f, 0.0, &["find", "-x", "/", "-name", "a"]);
    let found = judge_all(&[a, b, c, d, e, f, f_child]);
    assert_eq!(
        reasons(&found),
        vec![
            (1, Reason::WholeDisk("find /".into())),
            (2, Reason::WholeDisk("du ~".into())),
            (3, Reason::WholeDisk("find /Users/x".into())),
            (6, Reason::WholeDisk("find /".into())),
        ]
    );
}

#[test]
fn every_copy_but_the_oldest_is_a_duplicate() {
    let first = shell(1, 30, &side(1), "scripts/cargo.sh test -p a");
    let second = shell(2, 20, &side(1), "scripts/cargo.sh test  -p a");
    // The same command with the worktree named by a `cd`.
    let third = shell(
        3,
        10,
        &side(2),
        "cd ~/dev/demo-worktrees/side-1 && scripts/cargo.sh test -p a",
    );
    // The same command in another worktree, and another command here.
    let elsewhere = shell(4, 10, &side(2), "scripts/cargo.sh test -p a");
    let other = shell(5, 10, &side(1), "scripts/cargo.sh check");
    // Started a moment ago: may be run twice on purpose.
    let just_now = shell(6, 0, &side(1), "scripts/cargo.sh test -p a");
    let busy = |e: Entry| {
        let c = child(e.pid * 10, &e, 1000.0, &["cargo", "test"]);
        [e, c]
    };
    let mut all = Vec::new();
    for e in [first, second, third, elsewhere, other, just_now] {
        all.extend(busy(e));
    }
    let found = judge_all(&all);
    assert_eq!(found.running, 6);
    assert_eq!(
        reasons(&found),
        vec![
            (2, Reason::Duplicate { of: 1 }),
            (3, Reason::Duplicate { of: 1 })
        ]
    );
    assert_eq!(found.junk[0].worktree, "demo/side-1");
    assert_eq!(found.junk[1].command, "scripts/cargo.sh test -p a");
}

#[test]
fn a_shell_that_used_next_to_no_cpu_for_long_is_idle() {
    let stuck = shell(1, 171, &side(1), "sed -n 85,115p");
    // 30 minutes old: 5 CPU seconds is 0.3% of a core.
    let lock_wait = shell(2, 30, &side(1), "scripts/cargo.sh check");
    let lock_wait_child = child(21, &lock_wait, 5.0, &["cargo", "check"]);
    let young = shell(3, 5, &side(1), "scripts/cargo.sh clippy");
    let busy = shell(4, 90, &side(1), "scripts/cargo.sh build --release");
    let busy_cargo = child(41, &busy, 20.0, &["cargo", "build"]);
    let busy_rustc = child(42, &busy_cargo, 4000.0, &["rustc", "--crate-name", "a"]);
    let found = judge_all(&[
        stuck,
        lock_wait,
        lock_wait_child,
        young,
        busy,
        busy_cargo,
        busy_rustc,
    ]);
    let idle: Vec<(i32, i64)> = found.junk.iter().map(|j| (j.pid, j.minutes)).collect();
    assert_eq!(idle, vec![(1, 171), (2, 30)]);
    assert!(
        found
            .junk
            .iter()
            .all(|j| matches!(j.reason, Reason::Idle { percent } if percent < 2.0))
    );
}

/// The Claude Code process <pid> that started <shell>.
fn started_by(pid: i32, mut shell: Entry) -> Entry {
    shell.ppid = pid;
    shell
}

#[test]
fn a_watch_that_a_live_session_waits_on_is_not_idle() {
    // A watch uses next to no CPU on purpose, so a shell that a live session
    // waits on is not idle, whatever its CPU use.
    let command = "gh run watch 44466974160 --exit-status";
    let watch = started_by(4000, shell(1, 90, &side(1), command));
    let gh = child(
        11,
        &watch,
        0.4,
        &["gh", "run", "watch", "44466974160", "--exit-status"],
    );
    let entries = [watch, gh];
    let live = HashSet::from([4000]);
    let found = judge(
        &entries,
        NOW,
        Path::new(HOME),
        &places(),
        &HashSet::new(),
        &live,
    );
    assert!(found.junk.is_empty(), "{} junk", found.junk.len());
    assert_eq!(found.active, vec!["demo/side-1".to_string()]);
}

#[test]
fn the_same_watch_under_an_ended_session_is_idle() {
    let command = "gh run watch 44466974160 --exit-status";
    let watch = started_by(4000, shell(1, 90, &side(1), command));
    let gh = child(
        11,
        &watch,
        0.4,
        &["gh", "run", "watch", "44466974160", "--exit-status"],
    );
    let entries = [watch, gh];
    // Another session runs; the one that started the watch does not.
    let live = HashSet::from([4001]);
    let found = judge(
        &entries,
        NOW,
        Path::new(HOME),
        &places(),
        &HashSet::new(),
        &live,
    );
    assert_eq!(found.junk.len(), 1);
    assert!(matches!(found.junk[0].reason, Reason::Idle { .. }));
}

#[test]
fn a_wait_in_the_app_build_queue_is_not_idle() {
    let queued = shell(1, 60, &side(1), "builds app-build demo work macos");
    let queued_child = child(
        11,
        &queued,
        0.05,
        &["builds", "app-build", "demo", "work", "macos"],
    );
    let entries = [queued, queued_child];
    let waiting = HashSet::from([11]);
    let found = judge(
        &entries,
        NOW,
        Path::new(HOME),
        &places(),
        &waiting,
        &HashSet::new(),
    );
    assert!(found.junk.is_empty(), "{} junk", found.junk.len());
}

#[test]
fn a_queued_app_build_the_queue_list_missed_is_not_idle_either() {
    // `builds health` once judged this one idle at 0.2% of a core and
    // stopped it: it runs a build, so its CPU says nothing.
    let queued = shell(1, 60, &side(1), "builds app-build demo work macos");
    let queued_child = child(
        11,
        &queued,
        0.05,
        &["builds", "app-build", "demo", "work", "macos"],
    );
    let found = judge_all(&[queued, queued_child]);
    assert!(found.junk.is_empty(), "{} junk", found.junk.len());
}

#[test]
fn the_dashboard_and_the_forked_shell_of_a_command_are_not_shells_of_their_own() {
    let dashboard = shell(1, 600, &side(1), "builds dashboard");
    let parts = shell(
        2,
        40,
        &side(1),
        "cd ~/dev/demo-worktrees/side-1 && (scripts/cargo.sh test a; scripts/cargo.sh test b)",
    );
    let mut fork = child(
        21,
        &parts,
        900.0,
        &parts.argv.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    fork.argv = parts.argv.clone();
    let found = judge_all(&[dashboard, parts, fork]);
    assert_eq!(found.running, 2);
    assert!(found.junk.is_empty(), "{:?}", reasons(&found));
}

#[test]
fn the_lines_say_the_count_and_each_junk_shell() {
    let long = "x".repeat(300);
    let sleeper = shell(7, 171, &side(1), &format!("sleep 598 # {long}"));
    let sleep = child(71, &sleeper, 0.0, &["sleep", "598"]);
    let found = judge_all(&[
        sleeper,
        sleep,
        shell(8, 5, Path::new("/tmp"), "cargo check"),
    ]);
    let lines = lines(&found);
    assert_eq!(lines[0], "shells: 2 running, 1 junk");
    assert!(
        lines[1].starts_with(
            "    pid 7 (agent unknown, started by pid 0) 2h51m  demo/side-1  waits (sleep): sleep 598 # xxx"
        ),
        "{}",
        lines[1]
    );
    assert!(lines[1].ends_with('…'));
    assert_eq!(lines.len(), 2);
    assert_eq!(
        judge_all(&[shell(9, 5, Path::new("/tmp"), "ls")])
            .junk
            .len(),
        0
    );
}

#[test]
fn a_cargo_with_a_compile_in_flight_is_not_idle() {
    // The compile runs in the sccache server's tree; the client under cargo
    // uses no CPU.
    let compiling = shell(1, 40, &side(1), "scripts/cargo.sh check -p a");
    let cargo = child(11, &compiling, 1.0, &["cargo", "check"]);
    let client = child(
        12,
        &cargo,
        0.0,
        &["sccache", "/toolchain/bin/rustc", "--crate-name", "a"],
    );
    // The same cargo blocked on another's build-directory lock: no child.
    let blocked = shell(2, 40, &side(2), "scripts/cargo.sh check -p a");
    let blocked_cargo = child(21, &blocked, 1.0, &["cargo", "check"]);
    let found = judge_all(&[compiling, cargo, client, blocked, blocked_cargo]);
    assert_eq!(
        found.junk.iter().map(|j| j.pid).collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn the_checkouts_of_shells_that_are_not_junk_are_active() {
    let busy = shell(1, 90, &side(1), "scripts/cargo.sh build");
    let busy_cargo = child(11, &busy, 4000.0, &["cargo", "build"]);
    let stuck = shell(2, 171, &side(2), "sed -n 85,115p");
    let found = judge_all(&[busy, busy_cargo, stuck]);
    assert_eq!(found.active, vec!["demo/side-1".to_string()]);
    assert_eq!(found.junk.len(), 1);
}

#[test]
fn the_agent_comes_from_the_environment_or_the_command_text() {
    let env = |vars: &[&str]| vars.iter().map(OsString::from).collect::<Vec<_>>();
    assert_eq!(
        agent_in_environment(&env(&["A=1", "BUILDS_AGENT=a7f"])),
        Some("a7f".to_string())
    );
    assert_eq!(agent_in_environment(&env(&["BUILDS_AGENT="])), None);
    assert_eq!(agent_in_environment(&env(&["OTHER_BUILDS_AGENT=x"])), None);
    let text = |t: &str| agent_in_text(t);
    assert_eq!(
        text("cd /w && BUILDS_AGENT=ac0ffee scripts/cargo.sh test"),
        Some("ac0ffee".to_string())
    );
    assert_eq!(
        text("export BUILDS_AGENT=a09c; git log"),
        Some("a09c".to_string())
    );
    assert_eq!(
        text("export BUILDS_AGENT=\"a1b2\" && ls"),
        Some("a1b2".to_string())
    );
    assert_eq!(
        text("BUILDS_AGENT=e9a7-5b05_x ls"),
        Some("e9a7-5b05_x".to_string())
    );
    assert_eq!(text("echo $BUILDS_AGENT; sleep 5"), None);
    assert_eq!(text("MY_BUILDS_AGENT=a1 ls"), None);
    assert_eq!(text("sleep 590; date +%s"), None);
}

#[test]
fn a_junk_shell_names_its_agent_on_both_lines() {
    let polling = shell(
        7,
        3,
        &side(1),
        "cd /w && export BUILDS_AGENT=a77 && sleep 590; date +%s",
    );
    let sleep = child(71, &polling, 0.0, &["sleep", "590"]);
    let found = judge_all(&[polling, sleep]);
    assert_eq!(found.junk[0].agent.as_deref(), Some("a77"));
    assert!(
        lines(&found)[1].starts_with("    pid 7 (agent a77, started by pid 0) 3m"),
        "{}",
        lines(&found)[1]
    );
    assert!(
        stopped_line(&found.junk[0], 2)
            .starts_with("stopped pid 7 and 1 processes under it (agent a77, started by pid 0, "),
        "{}",
        stopped_line(&found.junk[0], 2)
    );
}

#[test]
fn an_agent_in_the_environment_of_a_process_in_the_tree_names_the_agent() {
    let polling = shell(7, 3, &side(1), "sleep 590");
    let mut sleep = child(71, &polling, 0.0, &["sleep", "590"]);
    sleep.agent = Some("a9e".to_string());
    let found = judge_all(&[polling, sleep]);
    assert_eq!(found.junk[0].agent.as_deref(), Some("a9e"));
}

#[test]
fn a_read_with_a_timeout_or_a_timeout_around_a_sleep_is_a_wait() {
    let kinds = [
        ("read -t 165 x < <(sleep 165)", waits("read -t")),
        (
            "timeout 170 bash -c 'read -t 165 x < <(sleep 165)'",
            waits("timeout around a sleep"),
        ),
        (
            "timeout 60 bash -c 'echo a; sleep 50; echo b'",
            waits("timeout around a sleep"),
        ),
    ];
    for (command, reason) in kinds {
        let found = judge_all(&[shell(7, 3, &side(1), command)]);
        assert_eq!(reasons(&found), vec![(7, reason)], "{command}");
    }
    for command in [
        "read x < file",
        "timeout 60 cargo test",
        "timeout 5 bash -c 'echo hi'",
    ] {
        assert!(
            judge_all(&[shell(7, 3, &side(1), command)]).junk.is_empty(),
            "{command}"
        );
    }
}

#[test]
fn a_perl_ruby_or_python_sleep_is_a_wait_with_its_agent() {
    // perl's four-argument select sleeps for its last argument's seconds.
    let forms: [(&str, &[&str], &str); 6] = [
        (
            "perl -e 'select(undef,undef,undef,170)'; tail -15 out",
            &["perl", "-e", "select(undef,undef,undef,170)"],
            "perl select",
        ),
        (
            "perl -e 'select(undef, undef, undef, 170)'",
            &["perl", "-e", "select(undef, undef, undef, 170)"],
            "perl select",
        ),
        (
            "perl -e 'sleep 170'",
            &["perl", "-e", "sleep 170"],
            "perl sleep",
        ),
        (
            "ruby -e 'sleep 170'",
            &["ruby", "-e", "sleep 170"],
            "ruby sleep",
        ),
        (
            "python3 -c 'import time; time.sleep(170)'",
            &["python3", "-c", "import time; time.sleep(170)"],
            "time.sleep",
        ),
        (
            "gtimeout 300 perl -e 'sleep 170'",
            &["gtimeout", "300", "perl", "-e", "sleep 170"],
            "perl sleep",
        ),
    ];
    for (command, argv, by) in forms {
        let top = shell(7, 3, &side(1), command);
        let mut sleeper = child(71, &top, 0.0, argv);
        sleeper.agent = Some("a9e".to_string());
        let found = judge_all(&[top, sleeper]);
        assert_eq!(reasons(&found), vec![(7, waits(by))], "{command}");
        assert_eq!(found.junk[0].agent.as_deref(), Some("a9e"), "{command}");
        assert!(
            lines(&found)[1].contains(&format!("waits ({by})")),
            "{command}"
        );
    }
    let work = shell(8, 3, &side(1), "perl -e 'print 1'");
    let found = judge_all(&[
        work.clone(),
        child(81, &work, 0.0, &["perl", "-e", "print 1"]),
    ]);
    assert!(found.junk.is_empty());
}

#[test]
fn a_gtimeout_or_timeout_around_a_sleeping_script_is_a_wait() {
    for command in [
        "gtimeout 170 bash -c 'read -t 165 x < <(sleep 165)'",
        "gtimeout 60 perl -e 'select(undef,undef,undef,50)'",
        "timeout -k 5 60 ruby -e 'sleep 50'",
    ] {
        let found = judge_all(&[shell(7, 3, &side(1), command)]);
        assert_eq!(
            reasons(&found),
            vec![(7, waits("timeout around a sleep"))],
            "{command}"
        );
    }
    assert!(
        judge_all(&[shell(7, 3, &side(1), "gtimeout 60 cargo test")])
            .junk
            .is_empty()
    );
}

#[test]
fn a_sleeping_shell_is_stopped_by_its_own_reason_not_as_a_duplicate() {
    let command = "perl -e 'select(undef,undef,undef,170)'; tail -15 out";
    let old = shell(1, 10, &side(1), command);
    let new = shell(2, 3, &side(1), command);
    let sleeper = child(
        21,
        &new,
        0.0,
        &["perl", "-e", "select(undef,undef,undef,170)"],
    );
    let found = judge_all(&[old, new, sleeper]);
    assert_eq!(reasons(&found), vec![(2, waits("perl select"))]);
}

/// <entries> judged while the Claude Code processes <sessions> run.
fn judge_with_sessions(entries: &[Entry], sessions: &[i32]) -> Shells {
    judge(
        entries,
        NOW,
        Path::new(HOME),
        &places(),
        &HashSet::new(),
        &sessions.iter().copied().collect(),
    )
}

/// A coordinator's shell (pid 1) that runs `claude -p`, the live agent session
/// (pid 2), the agent's own shell (pid 3, running <agent_command>) and, when
/// <search> is set, a `find /` under the agent's shell (pid 4).
fn coordinator_over_agent(minutes: i64, agent_command: &str, search: bool) -> Vec<Entry> {
    let outer = shell(
        1,
        minutes,
        &side(2),
        "claude -p --resume SID \"Find the cause of each group\"",
    );
    let session = child(2, &outer, 0.0, &["claude", "-p", "--resume", "SID"]);
    let mut inner = shell(3, minutes, &side(1), agent_command);
    inner.ppid = 2;
    let mut entries = vec![outer, session, inner.clone()];
    if search {
        entries.push(child(4, &inner, 0.0, &["find", "/", "-name", "a"]));
    }
    entries
}

#[test]
fn a_search_by_the_agent_a_shell_runs_is_not_the_search_of_that_shell() {
    let entries = coordinator_over_agent(3, "scripts/search.sh", true);
    let found = judge_with_sessions(&entries, &[2]);
    assert_eq!(
        reasons(&found),
        vec![(3, Reason::WholeDisk("find /".into()))]
    );
}

#[test]
fn a_shell_that_runs_a_live_session_is_not_idle() {
    let entries = coordinator_over_agent(30, "cargo test", false);
    let found = judge_with_sessions(&entries, &[2]);
    assert!(found.junk.is_empty(), "{:?}", reasons(&found));
}
