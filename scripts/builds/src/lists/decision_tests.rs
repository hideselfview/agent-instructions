use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, db, draft, item, question, read, stamp};

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn new_decision(title: &str) -> NewDecision {
    NewDecision {
        title: title.into(),
        by: "person".into(),
        body: "why".into(),
        question: None,
        replaces: None,
        plan: None,
    }
}

#[test]
fn a_decision_records_its_items_and_dates_itself() {
    let db = db();
    let q = question(&db);
    let mut new = new_decision("Chose the first way");
    new.question = Some(q);
    let work = [
        draft("One two three four five"),
        draft("Six seven eight nine ten"),
    ];
    let added = add_decision(&db, &new, &work, &stamp(NOON)).unwrap();
    // The question is answered by the closing that names the decision.
    crate::lists::record::close(
        &db,
        q,
        crate::lists::How::Answered(added.decision),
        &stamp(NOON),
    )
    .unwrap();
    let lists = read(&db);
    let decision = lists.decision(added.decision).unwrap();
    assert_eq!(decision.work, added.items);
    assert_eq!(decision.date, crate::text::local_day(NOON));
    assert_eq!(decision.question, Some(q));
    for id in &added.items {
        assert_eq!(lists.item(*id).unwrap().from, Some(added.decision));
    }
}

#[test]
fn a_decision_refused_up_front_changes_nothing() {
    let db = db();
    let before = read(&db);
    for bad in [
        NewDecision {
            title: "two\nlines".into(),
            ..new_decision("x")
        },
        NewDecision {
            by: "".into(),
            ..new_decision("x")
        },
        NewDecision {
            body: " ".into(),
            ..new_decision("x")
        },
        NewDecision {
            question: Some(Id(9)),
            ..new_decision("x")
        },
        NewDecision {
            replaces: Some(Id(9)),
            ..new_decision("x")
        },
    ] {
        assert!(add_decision(&db, &bad, &[], &stamp(NOON)).is_err());
        assert_eq!(read(&db), before);
    }
}

#[test]
fn a_decision_may_replace_an_earlier_one_and_answer_only_a_question() {
    let db = db();
    let first = add_decision(&db, &new_decision("First"), &[], &stamp(NOON))
        .unwrap()
        .decision;
    let mut second = new_decision("Second");
    second.replaces = Some(first);
    assert!(add_decision(&db, &second, &[], &stamp(NOON)).is_ok());
    let i = item(&db, vec![]);
    let mut wrong = new_decision("Third");
    wrong.question = Some(i);
    assert!(refusal(add_decision(&db, &wrong, &[], &stamp(NOON))).contains("not a question"));
}

#[test]
fn every_named_constraint_of_the_decision_tables_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, row, v};
    use crate::lists::record::open;
    let db = db();
    let good = open(&db, crate::lists::Kind::Decision, &stamp(1)).unwrap();
    let spare = open(&db, crate::lists::Kind::Decision, &stamp(1)).unwrap();
    let decision = [
        ("id", v(good.0)),
        ("date", v("2026-11-13")),
        ("title", v("A decision")),
        ("decided_by", v("person")),
        ("body", v("why")),
    ];
    plant(&db, "decision", &decision).unwrap();
    let mut refused = Refusals::new(include_str!("decision.sql"));
    let spare_row = |over: &[(&'static str, rusqlite::types::Value)]| {
        row(&row(&decision, &[("id", v(spare.0))]), over)
    };
    refused.check(
        "decision_is_a_decision",
        plant(&db, "decision", &spare_row(&[("record_kind", v("item"))])),
    );
    for date in ["", "yesterday", "2026-13-01", "2026-02-30", "2026-1-5"] {
        refused.check(
            "decision_date_is_a_day",
            plant(&db, "decision", &spare_row(&[("date", v(date))])),
        );
    }
    for title in ["", " x", "a\nb"] {
        refused.check(
            "decision_title_is_one_line",
            plant(&db, "decision", &spare_row(&[("title", v(title))])),
        );
    }
    refused.check(
        "decision_decided_by_says_something",
        plant(&db, "decision", &spare_row(&[("decided_by", v(" "))])),
    );
    refused.check(
        "decision_body_says_something",
        plant(&db, "decision", &spare_row(&[("body", v(" "))])),
    );
    // A decision replaces only an older one: itself and a newer one are refused, so no circle can form.
    refused.check(
        "decision_replaces_an_older_one",
        plant(&db, "decision", &spare_row(&[("replaces", v(spare.0))])),
    );
    let newer = open(&db, crate::lists::Kind::Decision, &stamp(1)).unwrap();
    plant(
        &db,
        "decision",
        &row(&decision, &[("id", v(newer.0)), ("replaces", v(good.0))]),
    )
    .unwrap();
    refused.check(
        "decision_replaces_an_older_one",
        plant(&db, "decision", &spare_row(&[("replaces", v(newer.0))])),
    );
    refused.check_as(
        "decision_is_a_record",
        "FOREIGN KEY",
        plant(&db, "decision", &row(&decision, &[("id", v(900))])),
    );
    let plan = [
        ("decision_id", v(good.0)),
        ("path", v("plans/p.md")),
        ("number", v("D1")),
    ];
    refused.check(
        "decision_plan_path_says_something",
        plant(&db, "decision_plan", &row(&plan, &[("path", v(" "))])),
    );
    refused.check(
        "decision_plan_number_says_something",
        plant(&db, "decision_plan", &row(&plan, &[("number", v(""))])),
    );
    plant(&db, "decision_plan", &plan).unwrap();
    refused.done();
}
