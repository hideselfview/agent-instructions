use super::*;
use crate::review::judge::tests::{Reviewed, review_under, reviewed_under, stamp};

fn settle(r: &Reviewed, how: &str) {
    settle_finding(r, r.finding, how);
}

fn settle_finding(r: &Reviewed, finding: i64, how: &str) {
    r.demo
        .store
        .write(&stamp("judge"), "settle", |db| {
            record::settle(db, finding, how, "because", 1, "judge")
        })
        .unwrap();
}

fn held(r: &Reviewed) -> Option<Held> {
    held_landing(r, r.landing)
}

fn held_landing(r: &Reviewed, landing: i64) -> Option<Held> {
    r.demo
        .store
        .write(&stamp("judge"), "hold", |db| {
            match Ready::of(db, landing)? {
                Some(ready) => hold(db, &ready, &stamp("judge")).map(Some),
                None => Ok(None),
            }
        })
        .unwrap()
}

#[test]
fn an_audit_with_a_finding_to_settle_makes_no_item() {
    let r = reviewed_under("audit-unsettled", None, "audit:nov14-1");
    assert_eq!(held(&r), None);
    let conn = r.demo.store.open_existing().unwrap();
    assert!(record::landing(&conn, r.landing).unwrap().is_open());
}

#[test]
fn an_audit_with_a_confirmed_finding_becomes_one_item_held_on_the_stack() {
    let r = reviewed_under("audit-confirmed", None, "audit:nov14-1");
    settle(&r, "confirmed");
    let Some(Held::Item { item, entry }) = held(&r) else {
        panic!("the audit makes an item");
    };
    let conn = r.demo.store.open_existing().unwrap();
    assert!(!record::landing(&conn, r.landing).unwrap().is_open());
    assert!(
        record::open_progress(&conn).unwrap().is_empty(),
        "no open review lists the audit"
    );
    let lists = crate::lists::store::load_all(&r.demo.tool.places).unwrap();
    let lists = lists[0].lists.as_ref().unwrap();
    let made = lists.item(item).unwrap();
    assert_eq!(made.title, "Fix the confirmed findings of audit nov14");
    assert_eq!(made.waits, vec![Wait::Question(entry)]);
    assert_eq!(lists.question(entry).unwrap().kind, QuestionKind::Held);
    assert!(made.details.contains("a.rs"), "{}", made.details);
    // Settling again makes no second item.
    assert_eq!(held(&r), None);
}

#[test]
fn an_audit_whose_findings_are_all_dropped_ends_with_no_item() {
    let r = reviewed_under("audit-dropped", None, "audit:nov14-2");
    settle(&r, "dropped");
    assert_eq!(held(&r), Some(Held::NothingToFix));
    let conn = r.demo.store.open_existing().unwrap();
    assert!(!record::landing(&conn, r.landing).unwrap().is_open());
}

#[test]
fn a_branch_review_is_no_audit() {
    let r = reviewed_under("audit-branch", None, "branch:work");
    settle(&r, "confirmed");
    assert_eq!(held(&r), None);
}

#[test]
fn the_landings_of_one_audit_share_one_entry_and_one_item() {
    assert_eq!(name_of("audit:nov14-12"), Some("nov14"));
    assert_eq!(name_of("audit:nov14"), Some("nov14"));
    assert_eq!(name_of("audit:sweep-run"), Some("sweep-run"));
    assert_eq!(name_of("branch:work"), None);
    let r = reviewed_under("audit-joined", None, "audit:nov14-1");
    settle(&r, "confirmed");
    let Some(Held::Item { item, .. }) = held(&r) else {
        panic!("the first landing makes the item");
    };
    // The audit's next landing: the same finding stands for its findings.
    let (next, next_finding) = review_under(&r.demo, &r.side, None, "audit:nov14-2");
    settle_finding(&r, next_finding, "confirmed");
    assert_eq!(held_landing(&r, next), Some(Held::Joined { item }));
    let lists = crate::lists::store::load_all(&r.demo.tool.places).unwrap();
    let lists = lists[0].lists.as_ref().unwrap();
    assert_eq!(lists.questions.len(), 1, "one held entry");
    assert_eq!(
        lists
            .items
            .iter()
            .filter(|item| item.title.contains("audit"))
            .count(),
        1,
        "one item"
    );
    assert_eq!(
        lists
            .item(item)
            .unwrap()
            .details
            .matches("Finding ")
            .count(),
        2
    );
}
