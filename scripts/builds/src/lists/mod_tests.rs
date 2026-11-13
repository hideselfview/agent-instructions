use super::*;
use crate::lists::fixtures::{item, lists, question, stamp};

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
fn a_side_worktree_path_names_its_repo_and_checkout() {
    assert_eq!(
        side_worktree_parts("/Users/x/dev/bae-worktrees/side-12"),
        Some(("bae".to_string(), "side-12".to_string()))
    );
    for bad in [
        "bae-worktrees/side-1",
        "/dev/bae/side-1",
        "/dev/bae-worktrees/side-",
        "/dev/bae-worktrees/side-1a",
        "/dev/bae-worktrees/main",
        "/dev/-worktrees/side-1",
    ] {
        assert_eq!(side_worktree_parts(bad), None, "{bad}");
    }
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
    crate::lists::draft::close(&mut lists, q, How::Done, &stamp(1)).unwrap();
    let open = lists.locate(i).unwrap();
    assert_eq!((open.kind, matches!(open.standing, Standing::Open)), (Kind::Item, true));
    let closed = lists.locate(q).unwrap();
    assert_eq!((closed.kind, matches!(closed.standing, Standing::Closed(_))), (Kind::Question, true));
    assert!(lists.locate(Id(99)).is_none());
}

#[test]
fn wire_shapes_are_the_plans() {
    let wait: Vec<Wait> = serde_json::from_str(r#"[{"item":4},{"question":3},{"design":9},{"condition":"quiet"}]"#).unwrap();
    assert_eq!(wait, vec![Wait::Item(Id(4)), Wait::Question(Id(3)), Wait::Design(Id(9)), Wait::Condition("quiet".into())]);
    assert_eq!(serde_json::to_string(&How::MovedToStack(Id(2))).unwrap(), r#"{"moved-to-stack":2}"#);
    assert_eq!(serde_json::to_string(&How::Done).unwrap(), r#""done""#);
    assert_eq!(serde_json::to_string(&Audience::DesignSession).unwrap(), r#""design-session""#);
    // Unknown fields are refused.
    assert!(serde_json::from_str::<Stamp>(r#"{"at":1,"by":"x","extra":1}"#).is_err());
}
