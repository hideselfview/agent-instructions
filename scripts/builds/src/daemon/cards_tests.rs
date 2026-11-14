use super::*;
use crate::claim::Claim;
use crate::daemon::model::Activity;
use crate::side_git::SideGit;

fn card(
    state: Option<&'static str>,
    claim: Option<Claim>,
    uncommitted: Option<usize>,
) -> CheckoutPage {
    CheckoutPage {
        name: "side-3".into(),
        path: "/d/bae-worktrees/side-3".into(),
        root: "/v/bae/side-3".into(),
        live: true,
        main: false,
        branch: Some("branch".into()),
        app_checkout: false,
        app_lane: None,
        claim,
        git: Some(SideGit {
            head: None,
            ahead: Vec::new(),
            uncommitted,
            last_write: None,
            error: None,
        }),
        state,
        folders: None,
        total_kib: None,
        activity: Activity::default(),
        words: Words::default(),
    }
}

fn claim(note: Option<&str>) -> Option<Claim> {
    Some(Claim {
        who: "agent".into(),
        since: "2026-11-14 07:00".into(),
        note: note.map(str::to_string),
        note_state: None,
    })
}

#[test]
fn the_chip_names_only_a_worktree_with_an_agent_or_commits() {
    let chip = |state| words(&card(Some(state), claim(None), Some(0))).chip;
    assert_eq!(chip("ready-to-land"), Some("Ready to land"));
    assert_eq!(chip("working"), Some("Working"));
    assert_eq!(chip("held"), Some("Working"));
    assert_eq!(chip("winding-down"), Some("Working"));
    assert_eq!(chip("waiting-on-you"), Some("Waiting on you"));
    assert_eq!(chip("free"), None);
    let mut main = card(None, None, None);
    main.main = true;
    assert_eq!(words(&main).chip, None);
}

#[test]
fn the_line_is_the_agents_note_or_what_the_state_means() {
    let line = |state, note| words(&card(Some(state), claim(note), Some(0))).line;
    assert_eq!(
        line("working", Some("Measuring each screen read.")),
        "Measuring each screen read."
    );
    assert_eq!(line("working", None), "Working; its agent left no note.");
    assert_eq!(
        line("ready-to-land", Some("note")),
        "Waits to be landed by the coordinator."
    );
    assert_eq!(line("free", None), "Free: no claim, every commit landed.");
    assert_eq!(
        words(&card(Some("ready-to-land"), None, Some(0))).line,
        "Commits wait to land and no agent claims this worktree."
    );
    let mut main = card(None, None, None);
    main.main = true;
    assert_eq!(
        words(&main).line,
        "Your checkout. No agent works or builds here."
    );
}

#[test]
fn the_tree_says_clean_or_how_many_files_changed() {
    let tree = |n| words(&card(Some("working"), claim(None), n)).tree;
    assert_eq!(tree(Some(0)).as_deref(), Some("clean"));
    assert_eq!(tree(Some(1)).as_deref(), Some("1 file changed"));
    assert_eq!(tree(Some(7)).as_deref(), Some("7 files changed"));
    assert_eq!(tree(None), None);
    let mut broken = card(Some("working"), claim(None), Some(0));
    broken.git.as_mut().unwrap().error = Some("git failed".into());
    assert_eq!(words(&broken).tree, None);
}
