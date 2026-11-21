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
