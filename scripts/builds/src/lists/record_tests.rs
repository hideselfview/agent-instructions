use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, db, item, question, read, stamp};

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

#[test]
fn ids_are_bare_numbers() {
    assert_eq!(Id::parse("12").unwrap(), Id(12));
    assert_eq!(Id::parse_list("3,5,8").unwrap(), vec![Id(3), Id(5), Id(8)]);
    for bad in ["", "0", "-1", "1.5", "+4", "a", "1,,2"] {
        assert!(Id::parse(bad).is_err() && Id::parse_list(bad).is_err(), "{bad}");
    }
    let hash = format!("{:?}", Id::parse("#12").unwrap_err());
    assert!(hash.contains("bare numbers"), "{hash}");
    assert_eq!(Id(12).to_string(), "#12");
}

#[test]
fn how_a_record_may_leave_depends_on_its_kind() {
    assert!(How::Answered(Id(1)).fits(Kind::Question));
    assert!(!How::Answered(Id(1)).fits(Kind::Item));
    assert!(How::Landed(vec![crate::lists::fixtures::sha('a')]).fits(Kind::Item));
    assert!(How::Landed(vec![crate::lists::fixtures::sha('a')]).fits(Kind::Design));
    assert!(!How::Done.fits(Kind::Item));
    assert!(How::MovedToStack(Id(1)).fits(Kind::Item));
    assert!(!How::MovedToStack(Id(1)).fits(Kind::Question));
    assert!(!How::Done.fits(Kind::Decision));
}

#[test]
fn locate_finds_open_and_closed_records() {
    let db = db();
    let q = question(&db);
    let i = item(&db, vec![]);
    crate::lists::record::close(&db, q, How::Done, &stamp(1)).unwrap();
    let lists = read(&db);
    let open = lists.locate(i).unwrap();
    assert_eq!((open.kind, matches!(open.standing, Standing::Open)), (Kind::Item, true));
    let closed = lists.locate(q).unwrap();
    assert_eq!((closed.kind, matches!(closed.standing, Standing::Closed(_))), (Kind::Question, true));
    assert!(lists.locate(Id(99)).is_none());
}

#[test]
fn close_closes_the_record_and_refuses_what_does_not_fit() {
    let db = db();
    let q = question(&db);
    let i = item(&db, vec![]);
    assert!(refusal(close(&db, i, How::Done, &stamp(NOON))).contains("item cannot be closed as done"));
    assert!(refusal(close(&db, Id(99), How::Done, &stamp(NOON))).contains("not a record"));
    close(&db, q, How::Done, &stamp(NOON + 5)).unwrap();
    let lists = read(&db);
    assert!(lists.question(q).is_none());
    let closed = lists.closed_record(q).unwrap();
    assert_eq!((closed.how.clone(), closed.closed.clone()), (How::Done, stamp(NOON + 5)));
    assert!(refusal(close(&db, q, How::Done, &stamp(NOON))).contains("already closed (done)"));
}

#[test]
fn every_named_constraint_of_the_record_tables_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, decision_answering, row, v};
    let db = db();
    let mut refused = Refusals::new(include_str!("record.sql"));
    let record = [("id", v(50)), ("kind", v("item")), ("opened_at", v(1)), ("opened_by", v("a"))];
    plant(&db, "record", &record).unwrap();
    let other = |id: i64| row(&record, &[("id", v(id))]);
    refused.check("record_id_positive", plant(&db, "record", &row(&other(0), &[])));
    refused.check("record_kind_known", plant(&db, "record", &row(&other(51), &[("kind", v("note"))])));
    refused.check("record_opened_by_says_something", plant(&db, "record", &row(&other(51), &[("opened_by", v(" "))])));
    refused.check_as("record_id_with_kind", "UNIQUE constraint failed", plant(&db, "record", &row(&record, &[("kind", v("question"))])));
    // Closings: a question and an item to close, and a decision that answers one question.
    let q = question(&db);
    let i = item(&db, vec![]);
    let second_q = question(&db);
    let decision = decision_answering(&db, q);
    close(&db, q, How::Answered(decision), &stamp(1)).unwrap();
    let closing = [("record_id", v(q.0)), ("kind", v("question")), ("how", v("done")), ("at", v(1)), ("by", v("a"))];
    let item_closing = [("record_id", v(i.0)), ("kind", v("item")), ("how", v("dropped")), ("at", v(1)), ("by", v("a")), ("reason", v("r"))];
    let on_second = |over: &[(&'static str, rusqlite::types::Value)]| row(&row(&closing, &[("record_id", v(second_q.0))]), over);
    refused.check("closing_how_known", plant(&db, "closing", &on_second(&[("how", v("finished"))])));
    refused.check("closing_by_says_something", plant(&db, "closing", &on_second(&[("by", v(" "))])));
    refused.check_as(
        "closing_decision_answers_once",
        "UNIQUE constraint failed: closing.decision_id",
        plant(&db, "closing", &on_second(&[("how", v("answered")), ("decision_id", v(decision.0))])),
    );
    refused.check_as("closing_names_its_record", "FOREIGN KEY", plant(&db, "closing", &row(&closing, &[("record_id", v(i.0))])));
    refused.check("closing_how_fits_kind", plant(&db, "closing", &on_second(&[("how", v("landed"))])));
    refused.check("closing_answered_names_its_decision", plant(&db, "closing", &on_second(&[("how", v("answered"))])));
    refused.check(
        "closing_moved_names_its_question",
        plant(&db, "closing", &row(&item_closing, &[("how", v("moved-to-stack")), ("reason", v(rusqlite::types::Value::Null))])),
    );
    refused.check("closing_reason_goes_with_how", plant(&db, "closing", &row(&item_closing, &[("reason", v(rusqlite::types::Value::Null))])));
    refused.check("closing_reason_says_something", plant(&db, "closing", &row(&item_closing, &[("reason", v(" "))])));
    plant(&db, "closing", &item_closing).unwrap();
    refused.check_as("closing_with_how", "UNIQUE constraint failed: closing.record_id", plant(&db, "closing", &item_closing));
    // Commits belong to a landed closing, once.
    let landed = item(&db, vec![]);
    close(&db, landed, How::Landed(vec![crate::lists::fixtures::sha('a')]), &stamp(1)).unwrap();
    let commit = [("record_id", v(landed.0)), ("how", v("landed")), ("position", v(5)), ("sha", v(crate::lists::fixtures::sha('b')))];
    plant(&db, "closing_commit", &commit).unwrap();
    let again = |over: &[(&'static str, rusqlite::types::Value)]| row(&row(&commit, &[("position", v(6)), ("sha", v(crate::lists::fixtures::sha('c')))]), over);
    refused.check("closing_commit_of_a_landing", plant(&db, "closing_commit", &again(&[("how", v("dropped"))])));
    refused.check("closing_commit_position_not_negative", plant(&db, "closing_commit", &again(&[("position", v(-1))])));
    for sha in ["abc", &"A".repeat(40), &"g".repeat(40)] {
        refused.check("closing_commit_sha_is_a_commit", plant(&db, "closing_commit", &again(&[("sha", v(sha.to_string()))])));
    }
    refused.check_as(
        "closing_commit_once",
        "UNIQUE constraint failed: closing_commit.record_id, closing_commit.sha",
        plant(&db, "closing_commit", &again(&[("sha", v(crate::lists::fixtures::sha('b')))])),
    );
    refused.check_as("closing_commit_belongs_to_a_landed_closing", "FOREIGN KEY", plant(&db, "closing_commit", &row(&again(&[]), &[("record_id", v(i.0))])));
    refused.done();
}
