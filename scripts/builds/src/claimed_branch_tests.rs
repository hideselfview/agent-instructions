use super::*;

fn claim() -> Claim {
    Claim {
        who: "agent-7".into(),
        since: 0,
        note: None,
    }
}

fn names(branches: &[&str]) -> Vec<String> {
    branches.iter().map(|b| b.to_string()).collect()
}

/// A claim with no running item has no branch yet, so nothing is refused.
#[test]
fn a_claim_with_no_running_item_allows_every_branch() {
    assert_eq!(refusal("/w/side-2", &claim(), &[], Some("x")), None);
    assert_eq!(refusal("/w/side-2", &claim(), &[], None), None);
}

#[test]
fn the_claimed_branch_is_allowed() {
    let held = names(&["a", "b"]);
    assert_eq!(refusal("/w/side-2", &claim(), &held, Some("b")), None);
}

#[test]
fn another_branch_is_refused_naming_the_holder_and_branch() {
    let text = refusal("/w/side-2", &claim(), &names(&["a"]), Some("other")).unwrap();
    assert!(text.contains("agent-7") && text.contains("branch a,"));
    assert!(text.contains("is on branch other") && text.contains("builds new --for"));
}

#[test]
fn a_detached_head_is_refused() {
    let text = refusal("/w/side-2", &claim(), &names(&["a"]), None).unwrap();
    assert!(text.contains("detached HEAD"));
}

/// `builds land` detaches HEAD and runs builds commands (checks, cargo) that
/// inherit the acknowledgement mark; those pass, an agent's own command does not.
#[test]
fn a_command_that_another_builds_command_started_is_not_checked() {
    assert!(applies("land", false) && applies("cargo", false));
    assert!(!applies("cargo", true) && !applies("checks", true));
    assert!(!applies("status", false));
}

/// A commit hook in a main checkout runs with a session and no agent id, so
/// reading the caller there would fail; the check never reads it.
#[test]
fn a_main_checkout_passes_without_reading_the_agent() {
    let dir = std::env::temp_dir().join(format!("builds-test-claimed-main-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let init = std::process::Command::new("git")
        .current_dir(&dir)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(init.success());
    let dir = std::fs::canonicalize(&dir).unwrap();
    let result = require_claimed_branch(&dir, || panic!("read the agent in a main checkout"));
    std::fs::remove_dir_all(&dir).unwrap();
    result.unwrap();
}
