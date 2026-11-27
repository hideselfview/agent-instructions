//! A repo two projects hold: an agent runs each command that names the repo or
//! one of its checkouts with `--project` from a checkout whose claim names no
//! project, and without it from a claimed one.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

fn command(dir: &PathBuf, session: Option<&str>, args: &[&str]) -> Command {
    let mut command = Command::new(BUILDS);
    command
        .current_dir(dir)
        .env("BUILDS_ROOT", dir.join("volume"))
        .env("BUILDS_POWER_RECORD", dir.join("power"))
        .env("HOME", dir.join("home"))
        .env("BUILDS_LISTS", dir.join("lists"))
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("BUILDS_ACKNOWLEDGED")
        .env_remove("BUILDS_AGENT")
        .args(args);
    if let Some(session) = session {
        command.env("CLAUDE_CODE_SESSION_ID", session);
    }
    command
}

fn builds(dir: &PathBuf, session: Option<&str>, args: &[&str]) -> Output {
    command(dir, session, args).output().unwrap()
}

/// Runs `builds agent start` as Claude Code's SessionStart hook does when
/// session <session> starts, which registers its main agent.
fn start_session(dir: &PathBuf, session: &str) -> Output {
    use std::io::Write;
    let mut child = command(dir, Some(session), &["agent", "start"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            format!(
                r#"{{"session_id": "{session}", "hook_event_name": "SessionStart", "source": "startup"}}"#
            )
            .as_bytes(),
        )
        .unwrap();
    child.wait_with_output().unwrap()
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Reads every part of the text and acknowledges it, as agent s1 of session
/// s1 in <cwd> with <flags>.
fn acknowledge(dir: &PathBuf, cwd: &Path, flags: &[&str]) {
    let mut words = vec!["ack".to_string()];
    for _ in 0..20 {
        let mut args: Vec<&str> = words.iter().map(String::as_str).collect();
        args.extend(["--agent", "s1"]);
        args.extend(flags);
        let output = command(dir, Some("s1"), &args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let last = text.trim_end().lines().last().unwrap().trim();
        let Some(next) = last.strip_prefix("builds ack ") else {
            return;
        };
        words = std::iter::once("ack")
            .chain(
                next.split_whitespace()
                    .take_while(|word| *word != "--agent"),
            )
            .map(str::to_string)
            .collect();
    }
    panic!("the acknowledgement did not end");
}

#[test]
fn each_command_on_a_two_project_repo_takes_project_or_the_claims() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-test-project-flag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("volume")).unwrap();
    std::fs::create_dir_all(dir.join("home/dev/demo")).unwrap();
    let run = |args: &[&str]| {
        let output = builds(&dir, None, args);
        assert!(output.status.success(), "{args:?}: {output:?}");
    };
    for project in ["one", "two"] {
        run(&[
            "lists", "init", project, "--repos", "demo", "--by", "person",
        ]);
    }
    let main = dir.join("home/dev/demo");
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["config", "user.email", "test@example.com"]);
    git(&main, &["config", "user.name", "Test"]);
    std::fs::write(main.join(".checks"), "check true *.txt true\n").unwrap();
    git(&main, &["add", ".checks"]);
    git(&main, &["commit", "-q", "-m", "base"]);
    git(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let sides = ["side-1", "side-2"].map(|name| {
        let side = dir.join("home/dev/demo-worktrees").join(name);
        let path = side.display().to_string();
        git(&main, &["worktree", "add", "-q", "--detach", &path]);
        side
    });
    let [claimed, unclaimed] = sides;
    let (own, other) = (
        claimed.display().to_string(),
        unclaimed.display().to_string(),
    );
    run(&["claim", &own, "tester", "--project", "one"]);
    let started = start_session(&dir, "s1");
    assert!(started.status.success(), "{started:?}");
    acknowledge(&dir, &claimed, &[]);
    acknowledge(&dir, &unclaimed, &["--project", "two"]);

    let commands: &[&[&str]] = &[
        &["queue", "demo"],
        &["queue", "demo", "top", "1"],
        &["takeover", "demo"],
        &["lock", "demo", "--lane", "mac", "true"],
        &["build-app", "demo", "mac", "no-such-ref"],
        &["shots", "findings", "demo", "HEAD", "mac", "mac.a."],
        &["shots", "check-program", "demo", "HEAD", "mac", "mac.a."],
        &["app-build", "demo", "--list"],
        &["app-build", "--landing-coverage", &other, "HEAD", "HEAD"],
        &["app-checkout", "reset", "demo", "side-9"],
        &["app-checkout", "remove", "mac", &other],
        &[
            "app-checkout",
            "remove",
            "mac",
            "--host",
            "none",
            "/a/dev/demo-worktrees/side-9",
        ],
        &["remote", "run", "demo", "no-such-ref", "--", "true"],
        &["install-hook", &other],
        &["checks", "list", &other],
        &["note", &own, "building"],
        &["retire", &own],
        &["release", &other],
    ];
    for words in commands {
        // `lock`'s own flags go before the command it runs, and `remote
        // run`'s before `--`.
        let at = match words[0] {
            "lock" => 4,
            "remote" => 4,
            _ => words.len(),
        };
        let agent = |cwd: &Path, project: &[&str]| {
            let mut args = words.to_vec();
            args.splice(at..at, project.iter().copied());
            args.splice(0..0, ["--agent", "s1"]);
            said(
                &command(&dir, Some("s1"), &args)
                    .current_dir(cwd)
                    .output()
                    .unwrap(),
            )
        };
        let refused = agent(&unclaimed, &[]);
        assert!(refused.contains("two projects"), "{words:?}: {refused}");
        for (cwd, project) in [(&unclaimed, &["--project", "two"][..]), (&claimed, &[][..])] {
            let ran = agent(cwd, project);
            for refusal in [
                "two projects",
                "unknown flag",
                "usage: builds",
                "acknowledg",
            ] {
                assert!(!ran.contains(refusal), "{words:?} {project:?}: {ran}");
            }
        }
    }
    let missing = builds(&dir, None, &["queue", "demo", "--project", "three"]);
    assert!(
        said(&missing).contains("there are no lists for project three"),
        "{missing:?}"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}
