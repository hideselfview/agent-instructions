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
    }
}

fn judge_all(entries: &[Entry]) -> Shells {
    judge(entries, NOW, Path::new(HOME), &places(), &HashSet::new())
}

fn reasons(shells: &Shells) -> Vec<(i32, Reason)> {
    shells.junk.iter().map(|j| (j.pid, j.reason.clone())).collect()
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
    assert!(runs_the_dashboard("/Users/x/.local/bin/builds dashboard --port 7787"));
    assert!(!runs_the_dashboard("builds status"));
}

#[test]
fn commands_split_like_a_shell_reads_them() {
    let words = |text: &str| -> Vec<String> { commands(text).iter().map(|c| c.join(" ")).collect() };
    assert_eq!(words("until ! pgrep -f 'cargo test'; do sleep 5; done"), ["until ! pgrep -f cargo test", "sleep 5", "done"]);
    assert_eq!(words("for i in $(seq 1 3); do xcrun swiftc -parse f; done"), ["for i in $", "seq 1 3", "xcrun swiftc -parse f", "done"]);
    assert_eq!(words("git commit -m \"wait; sleep until while\""), ["git commit -m wait; sleep until while"]);
    assert_eq!(words("cat <<'EOF'\nsleep 9\nEOF\necho done"), ["cat", "echo done"]);
    assert_eq!(words("FOO=1 nohup sleep 3 # sleep 4"), ["sleep 3"]);
}

#[test]
fn a_wait_is_junk_whatever_form_it_takes() {
    let a = shell(1, 3, &side(1), "until ! pgrep -f 'cargo test'; do sleep 5; done");
    let b = shell(2, 3, &side(1), "cd /tmp && sleep 600");
    let c = shell(3, 3, &side(1), "python3 -c \"import time; time.sleep(598)\"");
    let c_child = child(31, &c, 0.0, &["python3", "-c", "import time; time.sleep(598)"]);
    let d = shell(4, 3, &side(1), "caffeinate -i -t 119");
    let e = shell(5, 3, &side(1), "caffeinate -w 4242");
    let f = shell(6, 3, &side(1), "lsof -p 4242 +r 5");
    let g = shell(7, 3, &side(1), "for i in $(seq 1 5000); do xcrun swiftc -parse f.swift; done");
    let h = shell(8, 3, &side(1), "scripts/run-tests.sh");
    let h_child = child(81, &h, 0.0, &["/bin/sleep", "30"]);
    let found = judge_all(&[a, b, c, c_child, d, e, f, g, h, h_child]);
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
fn text_that_only_mentions_a_wait_is_not_one() {
    let commit = shell(1, 3, &side(1), "git commit -m 'wait until it is done, sleep well' && cat <<'EOF'\nwhile true; do sleep 1; done\nEOF");
    let build = shell(2, 3, &side(1), "caffeinate -i cargo build");
    let reader = shell(3, 3, &side(1), "while read line; do echo $line; done < list");
    let grep = shell(4, 3, &side(1), "git log --grep sleep");
    assert!(judge_all(&[commit, build, reader, grep]).junk.is_empty());
}

#[test]
fn a_search_over_the_whole_disk_is_junk() {
    let a = shell(1, 3, &side(1), "find / -name '*.swift' -mtime -1");
    let b = shell(2, 3, &side(1), "du -sh ~");
    let c = shell(3, 3, &side(1), "cd /tmp && find /Users/x -name a");
    let d = shell(4, 3, &side(1), "find . -name '*.rs'");
    let e = shell(5, 3, &side(1), "du -sh /Users/x/dev/demo-worktrees/side-1/target");
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
    let third = shell(3, 10, &side(2), "cd ~/dev/demo-worktrees/side-1 && scripts/cargo.sh test -p a");
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
    assert_eq!(reasons(&found), vec![(2, Reason::Duplicate { of: 1 }), (3, Reason::Duplicate { of: 1 })]);
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
    let found = judge_all(&[stuck, lock_wait, lock_wait_child, young, busy, busy_cargo, busy_rustc]);
    let idle: Vec<(i32, i64)> = found.junk.iter().map(|j| (j.pid, j.minutes)).collect();
    assert_eq!(idle, vec![(1, 171), (2, 30)]);
    assert!(found.junk.iter().all(|j| matches!(j.reason, Reason::Idle { percent } if percent < 2.0)));
}

#[test]
fn a_wait_in_the_app_build_queue_is_not_idle() {
    let queued = shell(1, 60, &side(1), "builds app-build demo work macos");
    let queued_child = child(11, &queued, 0.05, &["builds", "app-build", "demo", "work", "macos"]);
    let stranger = shell(2, 60, &side(1), "builds app-build demo other macos");
    let stranger_child = child(21, &stranger, 0.05, &["builds", "app-build", "demo", "other", "macos"]);
    let entries = [queued, queued_child, stranger, stranger_child];
    let waiting = HashSet::from([11]);
    let found = judge(&entries, NOW, Path::new(HOME), &places(), &waiting);
    assert_eq!(found.junk.iter().map(|j| j.pid).collect::<Vec<_>>(), vec![2]);
}

#[test]
fn the_dashboard_and_the_forked_shell_of_a_command_are_not_shells_of_their_own() {
    let dashboard = shell(1, 600, &side(1), "builds dashboard");
    let parts = shell(2, 40, &side(1), "cd ~/dev/demo-worktrees/side-1 && (scripts/cargo.sh test a; scripts/cargo.sh test b)");
    let mut fork = child(21, &parts, 900.0, &parts.argv.iter().map(String::as_str).collect::<Vec<_>>());
    fork.argv = parts.argv.clone();
    let found = judge_all(&[dashboard, parts, fork]);
    assert_eq!(found.running, 2);
    assert!(found.junk.is_empty(), "{:?}", reasons(&found));
}

#[test]
fn the_lines_say_the_count_and_each_junk_shell() {
    let long = "x".repeat(300);
    let found = judge_all(&[
        shell(7, 171, &side(1), &format!("sleep 598 # {long}")),
        shell(8, 5, Path::new("/tmp"), "cargo check"),
    ]);
    let lines = lines(&found);
    assert_eq!(lines[0], "shells: 2 running, 1 junk");
    assert!(lines[1].starts_with("    pid 7 2h51m  demo/side-1  waits (sleep): sleep 598 # xxx"), "{}", lines[1]);
    assert!(lines[1].ends_with('…'));
    assert_eq!(lines.len(), 2);
    assert_eq!(judge_all(&[shell(9, 5, Path::new("/tmp"), "ls")]).junk.len(), 0);
}
