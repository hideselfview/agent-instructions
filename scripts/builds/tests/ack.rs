//! A whole `builds` run: builds refuses an agent's command (one with a Claude
//! Code session id) until the agent acknowledges the help texts and rules
//! that `builds ack` prints, and runs the person's command.

use std::path::PathBuf;
use std::process::{Command, Output};

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

fn builds(dir: &PathBuf, session: Option<&str>, args: &[&str]) -> Output {
    let mut command = Command::new(BUILDS);
    command
        .current_dir(dir)
        .env("BUILDS_ROOT", dir.join("volume"))
        .env("HOME", dir.join("home"))
        .env("BUILDS_LISTS", dir.join("lists"))
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("BUILDS_ACKNOWLEDGED")
        .args(args);
    if let Some(session) = session {
        command.env("CLAUDE_CODE_SESSION_ID", session);
    }
    command.output().unwrap()
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

    let agent = builds(&dir, Some("s1"), &["du", "home"]);
    assert!(!agent.status.success());
    let said = String::from_utf8_lossy(&agent.stderr);
    assert!(said.contains("acknowledgement"), "{said}");

    let help = builds(&dir, Some("s1"), &["help"]);
    assert!(help.status.success(), "{help:?}");

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
    let agent = ["--project", "bae", "--by", "agent"];

    // install refuses the extra arguments with its usage, which shows that
    // the gate let the command through to install.
    let install = builds(&dir, Some("s1"), &[&["install"][..], &agent].concat());
    let said = String::from_utf8_lossy(&install.stderr);
    assert!(said.contains("usage: builds install"), "{said}");

    let other = builds(&dir, Some("s1"), &[&["du", "home"][..], &agent].concat());
    assert!(!other.status.success());
    let said = String::from_utf8_lossy(&other.stderr);
    assert!(said.contains("another builds schema"), "{said}");

    std::fs::remove_dir_all(&dir).unwrap();
}
