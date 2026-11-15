use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{Refusals, db, item, question, row, stamp, v};
use crate::lists::record::open;

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

#[test]
fn every_named_constraint_of_the_tag_table_refuses_its_row() {
    let db = db();
    let asked = question(&db);
    let decision = open(&db, Kind::Decision, &stamp(1)).unwrap();
    let tag = [
        ("record_id", v(asked.0)),
        ("kind", v("question")),
        ("tag", v("an audit")),
    ];
    plant(&db, "record_tag", &tag).unwrap();
    let mut refused = Refusals::new(include_str!("tag.sql"));
    refused.check(
        "record_tag_of_a_question_or_an_item",
        plant(
            &db,
            "record_tag",
            &row(
                &tag,
                &[("record_id", v(decision.0)), ("kind", v("decision"))],
            ),
        ),
    );
    for text in ["", " x", "x ", "a\nb", "a\rb", "a\tb"] {
        refused.check(
            "record_tag_is_one_line",
            plant(&db, "record_tag", &row(&tag, &[("tag", v(text))])),
        );
    }
    refused.check_as(
        "record_tag_names_its_record",
        "FOREIGN KEY",
        plant(
            &db,
            "record_tag",
            &row(&tag, &[("kind", v("item")), ("tag", v("another audit"))]),
        ),
    );
    refused.done();
    assert!(
        plant(&db, "record_tag", &tag)
            .unwrap_err()
            .to_string()
            .contains("UNIQUE"),
        "a record carries a tag once"
    );
}

#[test]
fn tags_go_on_questions_and_items_open_or_closed_and_come_back_in_order() {
    let db = db();
    let asked = question(&db);
    let work = item(&db, Vec::new());
    add_tags(&db, asked, &["b audit".into(), "a audit".into()]).unwrap();
    add_tags(&db, asked, &["a audit".into()]).unwrap();
    add_tags(&db, work, &["b audit".into()]).unwrap();
    crate::lists::record::close(
        &db,
        work,
        crate::lists::How::Dropped("no".into()),
        &stamp(2),
    )
    .unwrap();
    add_tags(&db, work, &["c audit".into()]).unwrap();
    let tags = read(&db).unwrap();
    assert_eq!(tags[&asked], ["a audit", "b audit"]);
    assert_eq!(tags[&work], ["b audit", "c audit"]);
    remove_tags(&db, work, &["b audit".into()]).unwrap();
    assert_eq!(read(&db).unwrap()[&work], ["c audit"]);
    assert!(refusal(remove_tags(&db, work, &["b audit".into()])).contains("has no tag 'b audit'"));
    let decision = open(&db, Kind::Decision, &stamp(1)).unwrap();
    assert!(
        refusal(add_tags(&db, decision, &["x".into()]))
            .contains("only questions and items take tags")
    );
    assert!(refusal(add_tags(&db, Id(999), &["x".into()])).contains("is not a record"));
}

#[test]
fn a_tag_is_one_line_with_no_spaces_at_its_ends() {
    assert_eq!(
        parse_tags(vec!["screen-reads audit"]).unwrap(),
        ["screen-reads audit"]
    );
    for bad in ["", " ", " x", "x ", "a\nb", "a\tb"] {
        assert!(parse_tags(vec![bad]).is_err(), "{bad:?}");
    }
    assert!(wanted(&["a".into(), "b".into()], Some("b")));
    assert!(!wanted(&["a".into()], Some("b")));
    assert!(wanted(&[], None));
}
