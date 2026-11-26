use super::*;
use crate::fail::Fail;
use crate::lists::How;
use crate::lists::fixtures::{NOON, db, draft, item, question, read, stamp};
use crate::lists::record::close;

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Exit(code) => panic!("exit {code}"),
        error => error.text(),
    }
}

#[test]
fn a_draft_needs_five_to_seven_words_a_known_area_and_details() {
    let db = db();
    let lists = &db;
    assert!(refusal(draft("Too short").validate(lists)).contains("2 words"));
    assert!(
        refusal(draft("One two three four five six seven eight").validate(lists))
            .contains("8 words")
    );
    let mut elsewhere = draft("One two three four five");
    elsewhere.area = "Nowhere".into();
    let message = refusal(elsewhere.validate(lists));
    assert!(
        message.contains("'Nowhere'")
            && message.contains("Alpha; Beta")
            && message.contains("area add"),
        "{message}"
    );
    let mut empty = draft("One two three four five");
    empty.details = " ".into();
    assert!(refusal(empty.validate(lists)).contains("details"));
    assert!(draft("One two three four five").validate(lists).is_ok());
}

#[test]
fn a_draft_waits_only_on_open_records() {
    let db = db();
    let q = question(&db);
    let open = item(&db, vec![]);
    let gone = item(&db, vec![]);
    close(
        &db,
        gone,
        How::Landed(vec![crate::lists::fixtures::sha('a')]),
        &stamp(NOON),
    )
    .unwrap();
    let with = |wait: Wait| {
        let mut d = draft("One two three four five");
        d.waits = vec![wait];
        d.validate(&db)
    };
    assert!(with(Wait::Question(q)).is_ok());
    assert!(with(Wait::Item(open)).is_ok());
    assert!(with(Wait::condition("quiet", crate::lists::Ender::Coordinator)).is_ok());
    assert!(refusal(with(Wait::Item(gone))).contains("is closed (landed)"));
    assert!(refusal(with(Wait::Item(Id(99)))).contains("no record has"));
    assert!(refusal(with(Wait::Item(q))).contains("as a item, but it is a question"));
    assert!(
        refusal(with(Wait::condition(" ", crate::lists::Ender::Coordinator))).contains("condition")
    );
}

#[test]
fn a_wait_on_a_question_clears_its_why_for_blocking_nothing() {
    let db = db();
    let by_draft = question(&db);
    let by_wait = question(&db);
    let untouched = question(&db);
    let mut waiting = draft("One two three four five");
    waiting.waits = vec![Wait::Question(by_draft)];
    add_item(&db, &waiting, None, &stamp(NOON)).unwrap();
    let later = item(&db, vec![]);
    add_wait(&db, later, Wait::Question(by_wait)).unwrap();
    let lists = read(&db);
    let why = |id: Id| lists.question(id).unwrap().blocks_nothing.clone();
    assert_eq!(
        (why(by_draft), why(by_wait), why(untouched)),
        (None, None, Some("nothing waits".to_string()))
    );
}

#[test]
fn add_item_makes_a_stamped_item_with_the_next_id() {
    let db = db();
    let decision = crate::lists::record::open(&db, Kind::Decision, &stamp(NOON)).unwrap();
    let mut d = draft("One two three four five");
    d.priority = Some(Priority {
        level: Level::First,
        why: "why".into(),
    });
    let id = add_item(&db, &d, Some(decision), &stamp(NOON)).unwrap();
    let lists = read(&db);
    let made = lists.item(id).unwrap();
    assert_eq!(
        (made.id, made.origin, made.opened.clone()),
        (Id(2), Some(Id(1)), stamp(NOON))
    );
    assert_eq!(made.priority, d.priority);
    // A refused draft hands out no id.
    let before = read(&db);
    assert!(add_item(&db, &draft("Short"), None, &stamp(NOON)).is_err());
    assert_eq!(read(&db), before);
    assert_eq!(crate::lists::fixtures::next_id(&db), Id(3));
}

#[test]
fn a_draft_matching_an_open_item_is_refused_and_names_it() {
    let db = db();
    let first = draft("One two three four five");
    let id = add_item(&db, &first, None, &stamp(NOON)).unwrap();
    let message = refusal(add_item(&db, &first, None, &stamp(NOON)));
    assert!(
        message.contains(&id.to_string()) && message.contains("same title and details"),
        "{message}"
    );
    // A refused draft hands out no id.
    assert_eq!(crate::lists::fixtures::next_id(&db), Id(id.0 + 1));
    // Another title, or other details, is another item.
    assert!(add_item(&db, &draft("One two three four six"), None, &stamp(NOON)).is_ok());
    let mut other_details = draft("One two three four five");
    other_details.details = "other details".into();
    assert!(add_item(&db, &other_details, None, &stamp(NOON)).is_ok());
}

#[test]
fn a_draft_matching_a_closed_item_is_added() {
    let db = db();
    let first = draft("One two three four five");
    let id = add_item(&db, &first, None, &stamp(NOON)).unwrap();
    close(
        &db,
        id,
        How::Landed(vec![crate::lists::fixtures::sha('a')]),
        &stamp(NOON),
    )
    .unwrap();
    assert!(add_item(&db, &first, None, &stamp(NOON)).is_ok());
}

#[test]
fn an_edit_changes_named_fields_by_the_drafts_rules() {
    let db = db();
    let id = item(&db, vec![]);
    let edit = |f: &dyn Fn(&mut ItemEdit)| {
        let mut e = ItemEdit {
            title: None,
            area: None,
            details: None,
            brief: None,
            r#for: None,
        };
        f(&mut e);
        e
    };
    assert!(refusal(edit(&|_| {}).apply(&db, id, &stamp(NOON))).contains("names no field"));
    assert!(
        refusal(edit(&|e| e.title = Some("Short".into())).apply(&db, id, &stamp(NOON)))
            .contains("words")
    );
    assert!(
        refusal(edit(&|e| e.area = Some("Nowhere".into())).apply(&db, id, &stamp(NOON)))
            .contains("Nowhere")
    );
    assert!(
        refusal(edit(&|e| e.details = Some("".into())).apply(&db, id, &stamp(NOON)))
            .contains("details")
    );
    assert!(
        refusal(edit(&|e| e.details = Some("d".into())).apply(&db, Id(99), &stamp(NOON)))
            .contains("not an open item")
    );
    edit(&|e| {
        e.title = Some("A new title of five".into());
        e.area = Some("Beta".into());
        e.details = Some("new".into());
        e.brief = Some("b.md".into());
        e.r#for = Some(Audience::DesignSession);
    })
    .apply(&db, id, &stamp(NOON))
    .unwrap();
    let lists = read(&db);
    let it = lists.item(id).unwrap();
    assert_eq!(
        (
            it.title.as_str(),
            it.area.as_str(),
            it.details.as_str(),
            it.brief.as_deref(),
            it.r#for
        ),
        (
            "A new title of five",
            "Beta",
            "new",
            Some("b.md"),
            Audience::DesignSession
        )
    );
}

#[test]
fn every_named_constraint_of_the_item_tables_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, row, v};
    let db = db();
    let good = record::open(&db, Kind::Item, &stamp(1)).unwrap();
    let spare = record::open(&db, Kind::Item, &stamp(1)).unwrap();
    let question = question(&db);
    let item = [
        ("id", v(good.0)),
        ("title", v("A title of five words")),
        ("area", v("Alpha")),
        ("details", v("d")),
        ("audience", v("coordinator")),
    ];
    plant(&db, "item", &item).unwrap();
    let mut refused = Refusals::new(include_str!("item.sql"));
    let spare_row = |over: &[(&'static str, rusqlite::types::Value)]| {
        row(&row(&item, &[("id", v(spare.0))]), over)
    };
    refused.check(
        "item_is_an_item",
        plant(&db, "item", &spare_row(&[("record_kind", v("design"))])),
    );
    for title in ["", " x", "a\nb"] {
        refused.check(
            "item_title_is_one_line",
            plant(&db, "item", &spare_row(&[("title", v(title))])),
        );
    }
    refused.check_as(
        "item_area_exists",
        "FOREIGN KEY",
        plant(&db, "item", &spare_row(&[("area", v("Nowhere"))])),
    );
    refused.check(
        "item_details_say_something",
        plant(&db, "item", &spare_row(&[("details", v(" "))])),
    );
    refused.check(
        "item_audience_known",
        plant(&db, "item", &spare_row(&[("audience", v("everyone"))])),
    );
    refused.check(
        "item_brief_says_something",
        plant(&db, "item", &spare_row(&[("brief", v(" "))])),
    );
    refused.check(
        "item_origin_not_itself",
        plant(&db, "item", &spare_row(&[("origin", v(spare.0))])),
    );
    refused.check_as(
        "item_is_a_record",
        "FOREIGN KEY",
        plant(&db, "item", &row(&item, &[("id", v(900))])),
    );
    refused.check_as(
        "item_origin_is_a_record",
        "FOREIGN KEY",
        plant(&db, "item", &spare_row(&[("origin", v(900))])),
    );
    let priority = [
        ("item_id", v(good.0)),
        ("level", v("first")),
        ("why", v("w")),
    ];
    refused.check(
        "item_priority_level_known",
        plant(
            &db,
            "item_priority",
            &row(&priority, &[("level", v("middle"))]),
        ),
    );
    refused.check(
        "item_priority_why_says_something",
        plant(&db, "item_priority", &row(&priority, &[("why", v(" "))])),
    );
    let backlog = [
        ("item_id", v(good.0)),
        ("why", v("w")),
        ("at", v(1)),
        ("by", v("b")),
    ];
    refused.check(
        "item_backlog_why_says_something",
        plant(&db, "item_backlog", &row(&backlog, &[("why", v(" "))])),
    );
    refused.check(
        "item_backlog_by_says_something",
        plant(&db, "item_backlog", &row(&backlog, &[("by", v(" "))])),
    );
    refused.check(
        "item_wait_item_not_itself",
        plant(
            &db,
            "item_wait_item",
            &[("item_id", v(good.0)), ("on_item", v(good.0))],
        ),
    );
    refused.check(
        "item_wait_condition_says_something",
        plant(
            &db,
            "item_wait_condition",
            &[
                ("item_id", v(good.0)),
                ("condition", v(" ")),
                ("until", v("agent")),
            ],
        ),
    );
    refused.check(
        "item_wait_condition_ender_known",
        plant(
            &db,
            "item_wait_condition",
            &[
                ("item_id", v(good.0)),
                ("condition", v("a quiet machine")),
                ("until", v("person")),
            ],
        ),
    );
    refused.check_as(
        "item_wait_item_target_exists",
        "FOREIGN KEY",
        plant(
            &db,
            "item_wait_item",
            &[("item_id", v(good.0)), ("on_item", v(900))],
        ),
    );
    refused.check_as(
        "item_wait_question_target_exists",
        "FOREIGN KEY",
        plant(
            &db,
            "item_wait_question",
            &[("item_id", v(good.0)), ("on_question", v(900))],
        ),
    );
    refused.check_as(
        "item_wait_design_target_exists",
        "FOREIGN KEY",
        plant(
            &db,
            "item_wait_design",
            &[("item_id", v(good.0)), ("on_design", v(900))],
        ),
    );
    let _ = question;
    let note = [
        ("item_id", v(good.0)),
        ("at", v(1)),
        ("by", v("a")),
        ("text", v("t")),
    ];
    refused.check(
        "item_note_by_says_something",
        plant(&db, "item_note", &row(&note, &[("by", v(" "))])),
    );
    refused.check(
        "item_note_says_something",
        plant(&db, "item_note", &row(&note, &[("text", v(" "))])),
    );
    // A wait is a set; the key refuses the same one twice.
    let real = crate::lists::fixtures::item(&db, vec![]);
    plant(
        &db,
        "item_wait_item",
        &[("item_id", v(good.0)), ("on_item", v(real.0))],
    )
    .unwrap();
    assert!(
        plant(
            &db,
            "item_wait_item",
            &[("item_id", v(good.0)), ("on_item", v(real.0))]
        )
        .is_err()
    );
    refused.covered_without_a_name(&[
        "item_area_exists",
        "item_wait_item_target_exists",
        "item_wait_question_target_exists",
        "item_wait_design_target_exists",
    ]);
    refused.done();
}

fn to_backlog(db: &Connection, ids: &[Id]) -> Result<()> {
    let moves: Vec<(Id, String)> = ids.iter().map(|id| (*id, "later".to_string())).collect();
    move_to_backlog(db, &moves, &stamp(NOON))
}

#[test]
fn an_item_moves_to_the_backlog_with_its_why_and_who_moved_it() {
    let db = db();
    let id = item(&db, vec![]);
    to_backlog(&db, &[id]).unwrap();
    let lists = read(&db);
    assert_eq!(
        lists.item(id).unwrap().backlog,
        Some(Backlog {
            why: "later".into(),
            at: NOON,
            by: "coordinator".into()
        })
    );
    assert!(refusal(to_backlog(&db, &[id])).contains("is in the backlog already"));
}

#[test]
fn the_backlog_refuses_an_item_that_runs_keeps_a_worktree_or_waits_on_an_open_question() {
    let db = db();
    let running_item = item(&db, vec![]);
    crate::lists::running::start(
        &db,
        &[running_item],
        &[crate::lists::Running {
            worktree: "/d/bae-worktrees/side-1".into(),
            branch: "work".into(),
            brief: "plans/b.md".into(),
            since: stamp(NOON),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
    let message = refusal(to_backlog(&db, &[running_item]));
    assert!(
        message.contains("runs") && message.contains("item stop"),
        "{message}"
    );
    let kept_item = item(&db, vec![]);
    crate::lists::kept_work::keep(
        &db,
        kept_item,
        &[crate::lists::kept_work::KeptWork {
            worktree: "/d/bae-worktrees/side-2".into(),
            branch: "kept".into(),
            since: stamp(NOON),
        }],
    )
    .unwrap();
    assert!(refusal(to_backlog(&db, &[kept_item])).contains("keeps a worktree"));
    let q = question(&db);
    let asked = item(&db, vec![Wait::Question(q)]);
    let message = refusal(to_backlog(&db, &[asked]));
    assert!(
        message.contains(&format!("waits on open question {q}"))
            && message.contains("question answer"),
        "{message}"
    );
    assert!(refusal(to_backlog(&db, &[Id(99)])).contains("is not a record"));
    assert!(read(&db).items.iter().all(|item| item.backlog.is_none()));
}

#[test]
fn the_backlog_refuses_an_item_that_an_item_outside_it_waits_on_and_takes_both_together() {
    let db = db();
    let first = item(&db, vec![]);
    let second = item(&db, vec![Wait::Item(first)]);
    let message = refusal(to_backlog(&db, &[first]));
    assert!(
        message.contains(&second.to_string()) && message.contains("builds item backlog 1 2"),
        "{message}"
    );
    to_backlog(&db, &[first, second]).unwrap();
    assert!(read(&db).items.iter().all(|item| item.backlog.is_some()));
}

#[test]
fn a_draft_in_the_backlog_is_added_there_and_waits_on_no_open_question() {
    let db = db();
    let mut later = draft("One two three four five");
    later.backlog = Some("the person starts it".into());
    let id = add_item(&db, &later, None, &stamp(NOON)).unwrap();
    assert_eq!(
        read(&db).item(id).unwrap().backlog.as_ref().unwrap().why,
        "the person starts it"
    );
    let q = question(&db);
    later.title = "Six seven eight nine ten".into();
    later.waits = vec![Wait::Question(q)];
    assert!(refusal(later.validate(&db)).contains("cannot wait on open question"));
    later.waits = Vec::new();
    later.backlog = Some(" ".into());
    assert!(refusal(later.validate(&db)).contains("needs its why"));
}

#[test]
fn a_wait_that_would_break_the_backlog_is_refused() {
    let db = db();
    let in_queue = item(&db, vec![]);
    let later = item(&db, vec![]);
    to_backlog(&db, &[later]).unwrap();
    let q = question(&db);
    assert!(refusal(add_wait(&db, later, Wait::Question(q))).contains("is in the backlog"));
    assert!(refusal(add_wait(&db, in_queue, Wait::Item(later))).contains("is in the backlog"));
    add_wait(&db, later, Wait::Item(in_queue)).unwrap();
    let mut draft_in_queue = draft("One two three four five");
    draft_in_queue.waits = vec![Wait::Item(later)];
    assert!(refusal(draft_in_queue.validate(&db)).contains("is in the backlog"));
}
