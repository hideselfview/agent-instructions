use super::*;
use crate::fail::Fail;
use crate::lists::fixtures::{NOON, item, lists, question, stamp};

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
    assert!(How::Landed(vec!["a".into()]).fits(Kind::Item));
    assert!(How::Landed(vec!["a".into()]).fits(Kind::Design));
    assert!(!How::Done.fits(Kind::Item));
    assert!(How::MovedToStack(Id(1)).fits(Kind::Item));
    assert!(!How::MovedToStack(Id(1)).fits(Kind::Question));
    assert!(!How::Done.fits(Kind::Decision));
}

#[test]
fn locate_finds_open_and_closed_records() {
    let mut lists = lists();
    let q = question(&mut lists);
    let i = item(&mut lists, vec![]);
    crate::lists::record::close(&mut lists, q, How::Done, &stamp(1)).unwrap();
    let open = lists.locate(i).unwrap();
    assert_eq!((open.kind, matches!(open.standing, Standing::Open)), (Kind::Item, true));
    let closed = lists.locate(q).unwrap();
    assert_eq!((closed.kind, matches!(closed.standing, Standing::Closed(_))), (Kind::Question, true));
    assert!(lists.locate(Id(99)).is_none());
}

#[test]
fn close_moves_the_record_and_refuses_what_does_not_fit() {
    let mut lists = lists();
    let q = question(&mut lists);
    let i = item(&mut lists, vec![]);
    assert!(refusal(close(&mut lists, i, How::Done, &stamp(NOON))).contains("item cannot be closed as done"));
    assert!(refusal(close(&mut lists, Id(99), How::Done, &stamp(NOON))).contains("not a record"));
    close(&mut lists, q, How::Done, &stamp(NOON + 5)).unwrap();
    assert!(lists.question(q).is_none());
    let closed = lists.closed_record(q).unwrap();
    assert_eq!((closed.how.clone(), closed.closed.clone()), (How::Done, stamp(NOON + 5)));
    assert!(refusal(close(&mut lists, q, How::Done, &stamp(NOON))).contains("already closed (done)"));
}
