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
    let chip = |state| words(&card(Some(state), claim(None), Some(0)), &[], None).chip;
    assert_eq!(chip("ready-to-land"), Some("Ready to land"));
    assert_eq!(chip("working"), Some("Working"));
    assert_eq!(chip("held"), Some("Working"));
    assert_eq!(chip("winding-down"), Some("Working"));
    assert_eq!(chip("waiting-on-you"), Some("Waiting on you"));
    assert_eq!(chip("free"), None);
    let mut main = card(None, None, None);
    main.main = true;
    assert_eq!(words(&main, &[], None).chip, None);
}

#[test]
fn the_line_is_the_agents_note_or_what_the_state_means() {
    let line = |state, note| words(&card(Some(state), claim(note), Some(0)), &[], None).line;
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
        words(&card(Some("ready-to-land"), None, Some(0)), &[], None).line,
        "Commits wait to land and no agent claims this worktree."
    );
    let mut main = card(None, None, None);
    main.main = true;
    assert_eq!(
        words(&main, &[], None).line,
        "Your checkout. No agent works or builds here."
    );
}

#[test]
fn the_tree_says_clean_or_how_many_files_changed() {
    let tree = |n| words(&card(Some("working"), claim(None), n), &[], None).tree;
    assert_eq!(tree(Some(0)).as_deref(), Some("clean"));
    assert_eq!(tree(Some(1)).as_deref(), Some("1 file changed"));
    assert_eq!(tree(Some(7)).as_deref(), Some("7 files changed"));
    assert_eq!(tree(None), None);
    let mut broken = card(Some("working"), claim(None), Some(0));
    broken.git.as_mut().unwrap().error = Some("git failed".into());
    assert_eq!(words(&broken, &[], None).tree, None);
}

fn titled(state: &'static str, note: Option<&str>, titles: &[&str]) -> Words {
    let titles: Vec<String> = titles.iter().map(|t| t.to_string()).collect();
    words(&card(Some(state), claim(note), Some(0)), &titles, None)
}

#[test]
fn a_running_items_title_replaces_the_claim_note() {
    for state in [
        "working",
        "waiting-on-you",
        "held",
        "winding-down",
        "landing",
    ] {
        let card = titled(
            state,
            Some("Measuring each screen read."),
            &["Test the storage manager"],
        );
        assert_eq!(card.titles, ["Test the storage manager"], "{state}");
        assert_eq!(card.line, "", "{state} shows no note under the title");
    }
}

#[test]
fn two_running_items_show_both_titles() {
    let card = titled("working", Some("note"), &["First item", "Second item"]);
    assert_eq!(card.titles, ["First item", "Second item"]);
    assert_eq!(card.line, "");
}

#[test]
fn a_ready_to_land_card_with_a_title_keeps_its_landing_line_and_not_the_note() {
    let card = titled("ready-to-land", Some("note"), &["Test the storage manager"]);
    assert_eq!(card.chip, Some("Ready to land"));
    assert_eq!(card.titles, ["Test the storage manager"]);
    assert_eq!(card.line, "Waits to be landed by the coordinator.");
}

#[test]
fn a_card_with_no_running_item_shows_its_note_and_no_title() {
    let card = titled("working", Some("Measuring each screen read."), &[]);
    assert!(card.titles.is_empty());
    assert_eq!(card.line, "Measuring each screen read.");
}

fn waiting(state: &'static str, wait: ItemWait) -> Words {
    let titles = vec!["Reopen a copied library".to_string()];
    words(
        &card(Some(state), claim(Some("note")), Some(0)),
        &titles,
        Some(&wait),
    )
}

#[test]
fn an_item_that_waits_on_a_question_says_so_under_waiting_on_you() {
    let card = waiting(
        "waiting-on-you",
        ItemWait::Answer("When to reopen a copied library".into()),
    );
    assert_eq!(card.chip, Some("Waiting on you"));
    assert_eq!(
        card.line,
        "Waits on your answer: When to reopen a copied library"
    );
}

#[test]
fn an_item_that_waits_on_another_item_names_it_and_stays_working() {
    let card = waiting(
        "working",
        ItemWait::Item {
            id: 1320,
            title: "Store the library id".into(),
        },
    );
    assert_eq!(card.chip, Some("Working"));
    assert_eq!(card.line, "Waits on #1320: Store the library id");
}

#[test]
fn an_item_that_waits_on_a_design_review_names_the_change() {
    let card = waiting(
        "waiting-on-you",
        ItemWait::Review {
            id: 12,
            title: "Heading row".into(),
        },
    );
    assert_eq!(
        card.line,
        "Waits on your review of design change 12: Heading row"
    );
}

#[test]
fn a_design_branch_no_recorded_change_covers_says_it_waits_on_the_design_session() {
    let card = words(
        &card(Some("ready-to-land"), claim(Some("note")), Some(0)),
        &[],
        Some(&ItemWait::Unrecorded),
    );
    assert_eq!(card.chip, Some("Ready to land"));
    assert_eq!(
        card.line,
        "Waits on the design session to record its design change."
    );
}

#[test]
fn a_design_branch_with_no_running_item_is_titled_by_its_design_change() {
    let card = words(
        &card(Some("waiting-on-you"), claim(Some("note")), Some(0)),
        &[],
        Some(&ItemWait::Review {
            id: 1419,
            title: "AirPlay PIN sheet".into(),
        }),
    );
    assert_eq!(card.titles, ["AirPlay PIN sheet"]);
    assert_eq!(card.line, "Waits on your review of design change 1419");
}

#[test]
fn an_item_waiting_on_a_question_keeps_its_own_title_over_the_question() {
    let card = waiting("waiting-on-you", ItemWait::Answer("Which port?".into()));
    assert_eq!(card.titles, ["Reopen a copied library"]);
    assert_eq!(card.line, "Waits on your answer: Which port?");
}
