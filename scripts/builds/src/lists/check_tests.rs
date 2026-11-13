use super::*;
use crate::fail::Fail;
use crate::lists::decision::{self, NewDecision, add_decision};
use crate::lists::fixtures::{NOON, design, item, lists, question, stamp};
use crate::lists::{Audience, How, LandedPart, Level, Priority, Running, area, design as design_owner, item as item_owner, project, record, running, store};

fn faults_text(lists: &Lists) -> String {
    faults(lists).join("\n")
}

fn assert_fault(lists: &Lists, expected: &str) {
    let text = faults_text(lists);
    assert!(text.contains(expected), "expected a fault with '{expected}', got:\n{text}");
}

fn decision(lists: &mut Lists) -> Id {
    let new = NewDecision {
        title: "A decision".into(),
        by: "person".into(),
        body: "body".into(),
        question: None,
        replaces: None,
        plan: None,
    };
    add_decision(lists, &new, &[], &stamp(NOON)).unwrap().decision
}

#[test]
fn a_fixture_state_passes() {
    let mut lists = lists();
    let q = question(&mut lists);
    item(&mut lists, vec![Wait::Question(q), Wait::Condition("quiet".into())]);
    design(&mut lists);
    decision(&mut lists);
    assert_eq!(faults(&lists), Vec::<String>::new());
    assert!(check(&lists).is_ok());
}

#[test]
fn the_error_names_every_fault() {
    let mut lists = lists();
    project::plant_repos(&mut lists, vec![]);
    area::plant_area_listed_again(&mut lists, "Alpha");
    let Fail::Message(message) = check(&lists).unwrap_err() else { panic!() };
    assert!(message.contains("no repos") && message.contains("'Alpha' is listed twice"), "{message}");
}


#[test]
fn project_and_repo_names_are_folder_safe() {
    for bad in ["", ".hidden", "a/b", "..", "a b"] {
        assert!(project_name_fault(bad).is_some(), "{bad}");
    }
    for good in ["bae", "my-project_2", "a.b"] {
        assert!(project_name_fault(good).is_none(), "{good}");
    }
    let mut lists = lists();
    project::plant_repos(&mut lists, vec!["a/b".into(), "bae".into(), "bae".into()]);
    assert_fault(&lists, "not a repo directory name");
    assert_fault(&lists, "bae is listed twice");
}

#[test]
fn design_places_name_project_repos() {
    let mut lists = lists();
    project::plant_design_places(&mut lists, "other", vec!["x/*".into()]);
    assert_fault(&lists, "repo other, which is not one of the project's");
}

#[test]
fn design_sources_and_spec_files_name_project_repos_and_no_empty_glob() {
    let mut lists = lists();
    project::plant_design_sources(&mut lists, "other", vec!["x/*".into()]);
    assert_fault(&lists, "design sources name repo other");
    let mut lists = crate::lists::fixtures::lists();
    project::plant_spec_files(&mut lists, "bae", vec![" ".into()]);
    assert_fault(&lists, "empty glob among its spec files");
}

#[test]
fn a_boards_link_and_a_design_changes_approval_are_checked() {
    let mut lists = lists();
    let id = crate::lists::fixtures::design(&mut lists);
    design_owner::plant_board(&mut lists, id, "design/mocks/A.dc.html", crate::lists::design::Board { link: " ".into() });
    assert_fault(&lists, "board design/mocks/A.dc.html is no place of it");
    assert_fault(&lists, "empty canvas link");
    design_owner::approve(&mut lists, id, &crate::lists::Stamp { at: NOON, by: " ".into() }).unwrap();
    assert_fault(&lists, "approval records no one who wrote it");
}

#[test]
fn ids_are_unique_and_below_next_id() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    let mut twin = lists.item(a).unwrap().clone();
    twin.title = "Another title of five words".into();
    item_owner::plant_item(&mut lists, twin);
    assert_fault(&lists, &format!("{a} is the id of two records"));
    let mut lists = crate::lists::fixtures::lists();
    item(&mut lists, vec![]);
    record::plant_next_id(&mut lists, Id(1));
    assert_fault(&lists, "is not below next_id");
}

#[test]
fn ids_are_unique_across_open_and_closed() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    let copy = lists.item(a).unwrap().clone();
    crate::lists::record::close(&mut lists, a, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    item_owner::plant_item(&mut lists, copy);
    assert_fault(&lists, "is the id of two records");
}

#[test]
fn every_reference_names_a_record_of_the_right_kind() {
    let mut lists = lists();
    let q = question(&mut lists);
    let d = decision(&mut lists);
    // A wait names an item but the id is a question.
    item(&mut lists, vec![Wait::Item(q)]);
    assert_fault(&lists, &format!("names {q}, a question, where a item belongs"));
    // A wait on nothing.
    item(&mut lists, vec![Wait::Question(Id(99))]);
    assert_fault(&lists, "names #99, which no record has");
    // An origin that is a question.
    let i = item(&mut lists, vec![]);
    crate::lists::item::plant_origin(&mut lists, i, Some(q));
    assert_fault(&lists, "origin names");
    // A decision's references.
    decision::plant_question(&mut lists, d, i);
    decision::plant_work(&mut lists, d, vec![q]);
    decision::plant_replaces(&mut lists, d, Id(77));
    assert_fault(&lists, "decision");
    let text = faults_text(&lists);
    for expected in ["'s question names", "'s work names", "'s replaces names #77"] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
}

#[test]
fn a_reference_to_a_closed_record_holds() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    let b = item(&mut lists, vec![Wait::Item(a)]);
    crate::lists::record::close(&mut lists, a, How::Landed(vec!["abc".into()]), &stamp(NOON)).unwrap();
    assert_eq!(faults(&lists), Vec::<String>::new(), "{b}");
}

#[test]
fn an_open_item_waiting_on_a_record_closed_unmeetably_is_a_fault() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    item(&mut lists, vec![Wait::Item(a)]);
    crate::lists::record::close(&mut lists, a, How::Dropped("not needed".into()), &stamp(NOON)).unwrap();
    assert_fault(&lists, "can no longer be met");
}

#[test]
fn no_item_waits_on_itself_through_a_chain() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    let b = item(&mut lists, vec![Wait::Item(a)]);
    let c = item(&mut lists, vec![Wait::Item(b)]);
    crate::lists::item::add_wait(&mut lists, a, Wait::Item(c)).unwrap();
    assert_fault(&lists, "wait on each other in a circle");
    let mut lists = crate::lists::fixtures::lists();
    let a = item(&mut lists, vec![]);
    crate::lists::item::add_wait(&mut lists, a, Wait::Item(a)).unwrap();
    assert_fault(&lists, "circle");
    // A diamond is not a cycle.
    let mut lists = crate::lists::fixtures::lists();
    let a = item(&mut lists, vec![]);
    let b = item(&mut lists, vec![Wait::Item(a)]);
    let c = item(&mut lists, vec![Wait::Item(a)]);
    item(&mut lists, vec![Wait::Item(b), Wait::Item(c)]);
    assert_eq!(faults(&lists), Vec::<String>::new());
}

#[test]
fn an_open_question_needs_a_waiting_item_or_a_reason() {
    let mut lists = lists();
    let q = question(&mut lists);
    let first = lists.questions[0].id;
    crate::lists::fixtures::blocks_something(&mut lists, first);
    assert_fault(&lists, &format!("question {q} blocks no open item"));
    item(&mut lists, vec![Wait::Question(q)]);
    assert_eq!(faults(&lists), Vec::<String>::new());
    let first = lists.questions[0].id;
    crate::lists::fixtures::blocks_nothing_because(&mut lists, first, "  ");
    assert_fault(&lists, "empty blocks-nothing reason");
}

#[test]
fn a_question_waited_on_only_by_a_closed_item_needs_a_reason() {
    let mut lists = lists();
    let q = question(&mut lists);
    let first = lists.questions[0].id;
    crate::lists::fixtures::blocks_something(&mut lists, first);
    let i = item(&mut lists, vec![Wait::Question(q)]);
    crate::lists::record::close(&mut lists, i, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    assert_fault(&lists, "blocks no open item");
}

#[test]
fn titles_are_one_line_and_item_titles_five_to_seven_words() {
    assert!(item_title_fault("one two three four five").is_none());
    assert!(item_title_fault("one two three four five six seven").is_none());
    assert!(item_title_fault("one two three four").is_some());
    assert!(item_title_fault("one two three four five six seven eight").is_some());
    assert!(item_title_fault("one two three\nfour five").is_some());
    assert!(item_title_fault(" one two three four five").is_some());
    assert!(title_fault("").is_some());
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    crate::lists::item::plant_title(&mut lists, i, "Too short");
    assert_fault(&lists, "item titles are 5 to 7");
    let q = question(&mut lists);
    let first = lists.questions[0].id;
    crate::lists::question::edit(&mut lists, first, Some("two\nlines".into()), None, None).unwrap();
    assert_fault(&lists, &format!("question {q}: title"));
}

#[test]
fn every_area_an_item_names_exists_open_or_closed() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    item_owner::plant_area(&mut lists, i, "Gone");
    assert_fault(&lists, &format!("item {i} is in area 'Gone', which does not exist"));
    crate::lists::record::close(&mut lists, i, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    assert_fault(&lists, &format!("closed item {i} is in area 'Gone', which does not exist"));
    let mut lists = crate::lists::fixtures::lists();
    let j = item(&mut lists, vec![]);
    crate::lists::record::close(&mut lists, j, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    assert_eq!(faults(&lists), Vec::<String>::new());
}

#[test]
fn running_worktrees_are_side_worktrees_of_project_repos() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    let run = |worktree: &str| Running {
        worktree: worktree.into(),
        branch: "b".into(),
        brief: "plans/briefs/x.md".into(),
        since: stamp(NOON),
    };
    running::start(&mut lists, &[i], &[run("/dev/bae-worktrees/side-2")]).unwrap();
    assert_eq!(faults(&lists), Vec::<String>::new());
    running::start(&mut lists, &[i], &[run("/dev/other-worktrees/side-2")]).unwrap();
    assert_fault(&lists, "worktree of repo other");
    running::start(&mut lists, &[i], &[run("/dev/bae")]).unwrap();
    assert_fault(&lists, "not a side worktree");
    crate::lists::running::start(&mut lists, &[i], &[Running { branch: " ".into(), ..run("/dev/bae-worktrees/side-2") }]).unwrap();
    assert_fault(&lists, "no branch or no brief");
}

#[test]
fn priorities_briefs_and_landed_parts_are_whole() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    item_owner::set_priority(&mut lists, i, Some(Priority { level: Level::First, why: "".into() })).unwrap();
    item_owner::plant_brief(&mut lists, i, " ");
    crate::lists::landing::add_part(&mut lists, i, LandedPart { repo: "nope".into(), commits: vec![], at: 1 }).unwrap();
    let text = faults_text(&lists);
    for expected in ["priority with no why", "empty brief path", "landed part of repo 'nope'"] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
}

#[test]
fn a_record_leaves_only_the_ways_its_kind_can() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    record::plant_closing(&mut lists, i, How::Done, &stamp(NOON));
    assert_fault(&lists, &format!("closed item {i} left as done"));
    record::plant_how(&mut lists, i, How::Landed(vec![]));
    assert_fault(&lists, "landed with no commits");
    record::plant_how(&mut lists, i, How::MovedToStack(Id(50)));
    assert_fault(&lists, "move to the stack names #50");
    record::plant_how(&mut lists, i, How::Dropped(" ".into()));
    assert_fault(&lists, "with nothing said");
}

#[test]
fn how_names_the_decision_that_answered() {
    let mut lists = lists();
    let q = question(&mut lists);
    record::plant_closing(&mut lists, q, How::Answered(Id(40)), &stamp(NOON));
    assert_fault(&lists, "answer names #40");
    let d = decision(&mut lists);
    record::plant_how(&mut lists, q, How::Answered(d));
    assert_eq!(faults(&lists), Vec::<String>::new(), "{q}");
}

#[test]
fn decisions_are_dated_decided_and_have_a_body() {
    let mut lists = lists();
    let d = decision(&mut lists);
    decision::plant_date(&mut lists, d, "2026-13-01");
    decision::plant_decided_by(&mut lists, d, "");
    decision::plant_body(&mut lists, d, " ");
    let text = faults_text(&lists);
    for expected in ["not YYYY-MM-DD", "names no one who decided", "empty body"] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    decision::plant_replaces(&mut lists, d, d);
    assert_fault(&lists, "replaces itself");
}

#[test]
fn replacement_chains_do_not_loop() {
    let mut lists = lists();
    let a = decision(&mut lists);
    let b = decision(&mut lists);
    decision::plant_replaces(&mut lists, a, b);
    decision::plant_replaces(&mut lists, b, a);
    assert_fault(&lists, "revises itself through a chain");
}

#[test]
fn design_changes_name_repos_and_resolved_places() {
    let mut lists = lists();
    let d = design(&mut lists);
    design_owner::plant_repo(&mut lists, d, "nope");
    design_owner::plant_place(&mut lists, d, "design/mocks/A.dc.html", Resolution::Matches(" ".into()));
    design_owner::plant_place(&mut lists, d, "design/mocks/B.dc.html", Resolution::Item(Id(70)));
    design_owner::plant_no_app_place(&mut lists, d, " ");
    design_owner::plant_landing(&mut lists, d, crate::lists::DesignLanding { commits: vec![], at: 1 });
    let text = faults_text(&lists);
    for expected in [
        "repo nope",
        "matches with no how",
        "place design/mocks/B.dc.html names #70",
        "empty no-app-place reason",
        "landing with no commits",
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
}

#[test]
fn version_is_the_logs_last_line() {
    let mut lists = lists();
    store::plant_version(&mut lists, 5);
    assert_fault(&lists, "version is 5 but the log ends at 1");
    let mut lists = crate::lists::fixtures::lists();
    store::plant_log_line(&mut lists, crate::lists::LogLine { version: 1, at: 1, by: "x".into(), words: "w".into() });
    assert_fault(&lists, "does not follow");
}

#[test]
fn every_write_names_who_wrote() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    crate::lists::item::plant_opened_by(&mut lists, i, "");
    crate::lists::item::plant_audience(&mut lists, i, Audience::DesignSession);
    assert_fault(&lists, "records no one who wrote it");
}
