use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, design, item, lists, question, stamp};
use crate::lists::{Audience, Decision, How, LandedPart, Level, Priority, Running};

fn faults_text(lists: &Lists) -> String {
    faults(lists).join("\n")
}

fn assert_fault(lists: &Lists, expected: &str) {
    let text = faults_text(lists);
    assert!(text.contains(expected), "expected a fault with '{expected}', got:\n{text}");
}

fn decision(lists: &mut Lists) -> Id {
    let id = lists.alloc_id();
    lists.decisions.push(Decision {
        id,
        date: "2026-11-13".into(),
        title: "A decision".into(),
        by: "person".into(),
        body: "body".into(),
        question: None,
        work: Vec::new(),
        replaces: None,
        plan: None,
        recorded: stamp(NOON),
    });
    id
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
    lists.repos.clear();
    lists.areas.push("Alpha".into());
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
    lists.repos = vec!["a/b".into(), "bae".into(), "bae".into()];
    assert_fault(&lists, "not a repo directory name");
    assert_fault(&lists, "bae is listed twice");
}

#[test]
fn design_places_name_project_repos() {
    let mut lists = lists();
    lists.design_places.insert("other".into(), vec!["x/*".into()]);
    assert_fault(&lists, "repo other, which is not one of the project's");
}

#[test]
fn ids_are_unique_and_below_next_id() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    let mut twin = lists.item(a).unwrap().clone();
    twin.title = "Another title of five words".into();
    lists.items.push(twin);
    assert_fault(&lists, &format!("{a} is the id of two records"));
    let mut lists = crate::lists::fixtures::lists();
    item(&mut lists, vec![]);
    lists.next_id = Id(1);
    assert_fault(&lists, "is not below next_id");
}

#[test]
fn ids_are_unique_across_open_and_closed() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    let copy = lists.item(a).unwrap().clone();
    crate::lists::draft::close(&mut lists, a, How::Dropped("x".into()), &stamp(NOON)).unwrap();
    lists.items.push(copy);
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
    lists.item_mut(i).unwrap().from = Some(q);
    assert_fault(&lists, "origin names");
    // A decision's references.
    let dec = lists.decisions.iter_mut().find(|x| x.id == d).unwrap();
    dec.question = Some(i);
    dec.work = vec![q];
    dec.replaces = Some(Id(77));
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
    crate::lists::draft::close(&mut lists, a, How::Landed(vec!["abc".into()]), &stamp(NOON)).unwrap();
    assert_eq!(faults(&lists), Vec::<String>::new(), "{b}");
}

#[test]
fn an_open_item_waiting_on_a_record_closed_unmeetably_is_a_fault() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    item(&mut lists, vec![Wait::Item(a)]);
    crate::lists::draft::close(&mut lists, a, How::Dropped("not needed".into()), &stamp(NOON)).unwrap();
    assert_fault(&lists, "can no longer be met");
}

#[test]
fn no_item_waits_on_itself_through_a_chain() {
    let mut lists = lists();
    let a = item(&mut lists, vec![]);
    let b = item(&mut lists, vec![Wait::Item(a)]);
    let c = item(&mut lists, vec![Wait::Item(b)]);
    lists.item_mut(a).unwrap().waits.push(Wait::Item(c));
    assert_fault(&lists, "wait on each other in a circle");
    let mut lists = crate::lists::fixtures::lists();
    let a = item(&mut lists, vec![]);
    lists.item_mut(a).unwrap().waits.push(Wait::Item(a));
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
    lists.questions[0].blocks_nothing = None;
    assert_fault(&lists, &format!("question {q} blocks no open item"));
    item(&mut lists, vec![Wait::Question(q)]);
    assert_eq!(faults(&lists), Vec::<String>::new());
    lists.questions[0].blocks_nothing = Some("  ".into());
    assert_fault(&lists, "empty blocks-nothing reason");
}

#[test]
fn a_question_waited_on_only_by_a_closed_item_needs_a_reason() {
    let mut lists = lists();
    let q = question(&mut lists);
    lists.questions[0].blocks_nothing = None;
    let i = item(&mut lists, vec![Wait::Question(q)]);
    crate::lists::draft::close(&mut lists, i, How::Dropped("x".into()), &stamp(NOON)).unwrap();
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
    lists.item_mut(i).unwrap().title = "Too short".into();
    assert_fault(&lists, "item titles are 5 to 7");
    let q = question(&mut lists);
    lists.questions[0].title = "two\nlines".into();
    assert_fault(&lists, &format!("question {q}: title"));
}

#[test]
fn every_area_an_open_item_names_exists_but_a_closed_item_may_name_a_removed_one() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    lists.item_mut(i).unwrap().area = "Gone".into();
    assert_fault(&lists, "area 'Gone', which does not exist");
    crate::lists::draft::close(&mut lists, i, How::Dropped("x".into()), &stamp(NOON)).unwrap();
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
    lists.item_mut(i).unwrap().running = Some(run("/dev/bae-worktrees/side-2"));
    assert_eq!(faults(&lists), Vec::<String>::new());
    lists.item_mut(i).unwrap().running = Some(run("/dev/other-worktrees/side-2"));
    assert_fault(&lists, "worktree of repo other");
    lists.item_mut(i).unwrap().running = Some(run("/dev/bae"));
    assert_fault(&lists, "not a side worktree");
    lists.item_mut(i).unwrap().running = Some(Running { branch: " ".into(), ..run("/dev/bae-worktrees/side-2") });
    assert_fault(&lists, "no branch or no brief");
}

#[test]
fn priorities_briefs_and_landed_parts_are_whole() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    let it = lists.item_mut(i).unwrap();
    it.priority = Some(Priority { level: Level::First, why: "".into() });
    it.brief = Some(" ".into());
    it.landed.push(LandedPart { repo: "nope".into(), commits: vec![], at: 1 });
    let text = faults_text(&lists);
    for expected in ["priority with no why", "empty brief path", "landed part of repo 'nope'"] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
}

#[test]
fn a_record_leaves_only_the_ways_its_kind_can() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    let record = lists.items.remove(0);
    lists.closed.push(Closed {
        record: Record::Item(record),
        how: How::Done,
        closed: stamp(NOON),
    });
    assert_fault(&lists, &format!("closed item {i} left as done"));
    lists.closed[0].how = How::Landed(vec![]);
    assert_fault(&lists, "landed with no commits");
    lists.closed[0].how = How::MovedToStack(Id(50));
    assert_fault(&lists, "move to the stack names #50");
    lists.closed[0].how = How::Dropped(" ".into());
    assert_fault(&lists, "with nothing said");
}

#[test]
fn how_names_the_decision_that_answered() {
    let mut lists = lists();
    let q = question(&mut lists);
    let record = lists.questions.remove(0);
    lists.closed.push(Closed { record: Record::Question(record), how: How::Answered(Id(40)), closed: stamp(NOON) });
    assert_fault(&lists, "answer names #40");
    let d = decision(&mut lists);
    lists.closed[0].how = How::Answered(d);
    assert_eq!(faults(&lists), Vec::<String>::new(), "{q}");
}

#[test]
fn decisions_are_dated_decided_and_have_a_body() {
    let mut lists = lists();
    let d = decision(&mut lists);
    let dec = &mut lists.decisions[0];
    dec.date = "2026-13-01".into();
    dec.by = "".into();
    dec.body = " ".into();
    let text = faults_text(&lists);
    for expected in ["not YYYY-MM-DD", "names no one who decided", "empty body"] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    lists.decisions[0].replaces = Some(d);
    assert_fault(&lists, "replaces itself");
}

#[test]
fn replacement_chains_do_not_loop() {
    let mut lists = lists();
    let a = decision(&mut lists);
    let b = decision(&mut lists);
    lists.decisions[0].replaces = Some(b);
    lists.decisions[1].replaces = Some(a);
    assert_fault(&lists, "revises itself through a chain");
}

#[test]
fn design_changes_name_repos_and_resolved_places() {
    let mut lists = lists();
    let d = design(&mut lists);
    let change = lists.design_mut(d).unwrap();
    change.repo = "nope".into();
    change.places.insert("design/mocks/A.dc.html".into(), Resolution::Matches(" ".into()));
    change.places.insert("design/mocks/B.dc.html".into(), Resolution::Item(Id(70)));
    change.no_app_place = Some(" ".into());
    change.landed.push(crate::lists::DesignLanding { commits: vec![], at: 1 });
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
    lists.version = 5;
    assert_fault(&lists, "version is 5 but the log ends at 1");
    let mut lists = crate::lists::fixtures::lists();
    lists.log.push(crate::lists::LogLine { version: 1, at: 1, by: "x".into(), words: "w".into() });
    assert_fault(&lists, "does not follow");
}

#[test]
fn every_write_names_who_wrote() {
    let mut lists = lists();
    let i = item(&mut lists, vec![]);
    lists.item_mut(i).unwrap().opened.by = "".into();
    lists.item_mut(i).unwrap().r#for = Audience::DesignSession;
    assert_fault(&lists, "records no one who wrote it");
}
