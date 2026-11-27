//! A whole `builds` run: builds refuses an agent's command (one with a Claude
//! Code session id) until the agent acknowledges the workflow help and lists
//! overview that `builds ack` prints, and runs the person's command.

use std::path::PathBuf;
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

#[test]
fn an_agents_command_waits_for_its_acknowledgement_and_the_persons_runs() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-test-ack-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("volume")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();

    let person = builds(&dir, None, &["du", "home"]);
    assert!(person.status.success(), "{person:?}");

    let started = start_session(&dir, "s1");
    assert!(started.status.success(), "{started:?}");
    let told = String::from_utf8_lossy(&started.stdout);
    assert!(told.contains("Your builds agent id is s1."), "{told}");

    let unnamed = builds(&dir, Some("s1"), &["du", "home"]);
    assert!(!unnamed.status.success());
    let said = String::from_utf8_lossy(&unnamed.stderr);
    assert!(said.contains("pass --agent ID"), "{said}");
    assert!(said.contains("BUILDS_AGENT=ID"), "{said}");

    let agent = builds(&dir, Some("s1"), &["du", "home", "--agent", "s1"]);
    assert!(!agent.status.success());
    let said = String::from_utf8_lossy(&agent.stderr);
    assert!(said.contains("acknowledgement"), "{said}");

    let help = builds(&dir, Some("s1"), &["help"]);
    assert!(help.status.success(), "{help:?}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_agent_lists_the_checkouts_before_it_acknowledges_and_builds_refuses_its_other_commands() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-test-ack-status-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("volume")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let started = start_session(&dir, "s1");
    assert!(started.status.success(), "{started:?}");

    let listed = builds(
        &dir,
        Some("s1"),
        &["status", "--json", "--no-folders", "--agent", "s1"],
    );
    let said = String::from_utf8_lossy(&listed.stderr);
    assert!(listed.status.success(), "{said}");
    assert!(String::from_utf8_lossy(&listed.stdout).contains("\"repos\""));

    let folders = builds(&dir, Some("s1"), &["status", "--json", "--agent", "s1"]);
    let said = String::from_utf8_lossy(&folders.stderr);
    assert!(said.contains("acknowledgement"), "{said}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_agent_runs_install_on_a_store_of_an_older_schema_and_builds_refuses_its_other_commands() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-test-ack-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("volume")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let made = builds(
        &dir,
        None,
        &["lists", "init", "bae", "--repos", "bae", "--by", "person"],
    );
    assert!(made.status.success(), "{made:?}");
    rusqlite::Connection::open(dir.join("lists/bae/builds.sqlite"))
        .unwrap()
        .pragma_update(None, "user_version", 1)
        .unwrap();
    let started = start_session(&dir, "s1");
    assert!(started.status.success(), "{started:?}");
    let agent = ["--project", "bae", "--agent", "s1"];

    // install refuses the extra arguments with its usage, which shows that
    // the gate let the command through to install.
    let install = builds(
        &dir,
        Some("s1"),
        &[&["install", "extra"][..], &agent].concat(),
    );
    let said = String::from_utf8_lossy(&install.stderr);
    assert!(said.contains("usage: builds install"), "{said}");

    let other = builds(&dir, Some("s1"), &[&["du", "home"][..], &agent].concat());
    assert!(!other.status.success());
    let said = String::from_utf8_lossy(&other.stderr);
    assert!(said.contains("another builds schema"), "{said}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_commits_hook_in_the_main_checkout_of_a_repo_two_projects_hold_passes() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-test-ack-main-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("volume")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    for project in ["one", "two"] {
        let made = builds(
            &dir,
            None,
            &[
                "lists", "init", project, "--repos", "demo", "--by", "person",
            ],
        );
        assert!(made.status.success(), "{made:?}");
    }
    let repo = dir.join("demo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(&repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join(".checks"), "check true *.txt true\n").unwrap();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&["add", ".checks", "a.txt"]);

    let hook = builds(&repo, Some("s1"), &["checks", "run"]);
    assert!(hook.status.success(), "{hook:?}");

    let other = builds(&repo, Some("s1"), &["du", "a.txt"]);
    assert!(!other.status.success(), "{other:?}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_agent_without_a_current_acknowledgement_gets_a_refusal_from_cargo_and_no_output() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-test-ack-cargo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("volume")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let made = builds(
        &dir,
        None,
        &["lists", "init", "bae", "--repos", "bae", "--by", "person"],
    );
    assert!(made.status.success(), "{made:?}");
    let started = start_session(&dir, "s1");
    assert!(started.status.success(), "{started:?}");

    for args in [
        &[
            "cargo",
            "nextest",
            "run",
            "--project",
            "bae",
            "--agent",
            "s1",
        ][..],
        &["cargo", "test", "--project", "bae", "--agent", "s1"][..],
        &["cargo", "clippy", "--project", "bae", "--agent", "s1"][..],
    ] {
        let refused = builds(&dir, Some("s1"), args);
        assert_eq!(refused.status.code(), Some(1), "{args:?}: {refused:?}");
        assert!(refused.stdout.is_empty(), "{args:?}: {refused:?}");
        let said = String::from_utf8_lossy(&refused.stderr);
        assert!(
            said.contains("builds refuses this command until you read the text")
                && said.contains("builds ack --agent s1"),
            "{args:?}: {said}"
        );
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn dashboard_startup_children_do_not_inherit_the_installing_agents_identity() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "builds-test-dashboard-identity-{}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    let git = dir.join("git");
    std::fs::write(
        &git,
        r#"#!/bin/sh
printf '%s:%s
' "${CODEX_SESSION_ID-unset}" "${CODEX_THREAD_ID-unset}" > "$CAPTURED_IDENTITY"
exec /usr/bin/git "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(dir.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let captured = dir.join("captured");
    let output = command(&dir, None, &["dashboard", "--unknown"])
        .env("CODEX_SESSION_ID", "00000000-0000-4000-8000-000000000001")
        .env("CODEX_THREAD_ID", "00000000-0000-4000-8000-000000000002")
        .env("CAPTURED_IDENTITY", &captured)
        .env("PATH", path)
        .output()
        .unwrap();
    // Stop before binding a server. The actual dashboard startup already ran
    // the git child that lists git's local environment variables.
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage: builds dashboard"),
        "{output:?}"
    );
    let inherited = std::fs::read_to_string(captured).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(inherited, "unset:unset\n");
}

#[test]
fn the_rules_hook_exits_with_2_on_a_bad_agent_flag_and_other_commands_with_1() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-test-ack-hook-exit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("volume")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();

    let hook = builds(
        &dir,
        Some("s1"),
        &["--agent", "a1", "rules-on-edit", "--agent"],
    );
    assert_eq!(hook.status.code(), Some(2), "{hook:?}");
    let other = builds(
        &dir,
        Some("s1"),
        &["--agent", "a1", "du", "home", "--agent"],
    );
    assert_eq!(other.status.code(), Some(1), "{other:?}");

    std::fs::remove_dir_all(&dir).unwrap();
}
