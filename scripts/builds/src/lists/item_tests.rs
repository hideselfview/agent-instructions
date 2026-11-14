use super::*;
use crate::fail::Fail;
use crate::lists::How;
use crate::lists::fixtures::{NOON, db, draft, item, question, read, stamp};
use crate::lists::record::close;

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
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
    assert!(with(Wait::Condition("quiet".into())).is_ok());
    assert!(refusal(with(Wait::Item(gone))).contains("is closed (landed)"));
    assert!(refusal(with(Wait::Item(Id(99)))).contains("no record has"));
    assert!(refusal(with(Wait::Item(q))).contains("as a item, but it is a question"));
    assert!(refusal(with(Wait::Condition(" ".into()))).contains("condition"));
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
    let id = add_item(&db, &d, Some(Origin::Decision(decision)), &stamp(NOON)).unwrap();
    let lists = read(&db);
    let made = lists.item(id).unwrap();
    assert_eq!(
        (made.id, made.from, made.opened.clone()),
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
    assert!(refusal(edit(&|_| {}).apply(&db, id)).contains("names no field"));
    assert!(refusal(edit(&|e| e.title = Some("Short".into())).apply(&db, id)).contains("words"));
    assert!(refusal(edit(&|e| e.area = Some("Nowhere".into())).apply(&db, id)).contains("Nowhere"));
    assert!(refusal(edit(&|e| e.details = Some("".into())).apply(&db, id)).contains("details"));
    assert!(
        refusal(edit(&|e| e.details = Some("d".into())).apply(&db, Id(99)))
            .contains("not an open item")
    );
    edit(&|e| {
        e.title = Some("A new title of five".into());
        e.area = Some("Beta".into());
        e.details = Some("new".into());
        e.brief = Some("b.md".into());
        e.r#for = Some(Audience::DesignSession);
    })
    .apply(&db, id)
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
    let decision = record::open(&db, Kind::Decision, &stamp(1)).unwrap();
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
        "item_origin_kind_known",
        plant(
            &db,
            "item",
            &spare_row(&[("origin", v(decision.0)), ("origin_kind", v("item"))]),
        ),
    );
    refused.check(
        "item_origin_is_whole",
        plant(&db, "item", &spare_row(&[("origin", v(decision.0))])),
    );
    refused.check(
        "item_origin_is_whole",
        plant(&db, "item", &spare_row(&[("origin_kind", v("decision"))])),
    );
    refused.check_as(
        "item_is_a_record",
        "FOREIGN KEY",
        plant(&db, "item", &row(&item, &[("id", v(900))])),
    );
    refused.check_as(
        "item_origin_is_a_record",
        "FOREIGN KEY",
        plant(
            &db,
            "item",
            &spare_row(&[("origin", v(good.0)), ("origin_kind", v("decision"))]),
        ),
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
            &[("item_id", v(good.0)), ("condition", v(" "))],
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
