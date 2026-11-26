use super::*;
use crate::lists::Running;
use crate::lists::fixtures::{NOON, db, decision_answering, item, question, read, stamp};
use crate::lists::item::ItemEdit;

fn edit_details(db: &Connection, id: Id, details: &str, at: i64) {
    ItemEdit {
        title: None,
        area: None,
        details: Some(details.into()),
        brief: None,
        r#for: None,
    }
    .apply(db, id, &stamp(at))
    .unwrap();
}

fn take(db: &Connection, id: Id, at: i64) {
    crate::lists::running::start(
        db,
        &[id],
        &[Running {
            worktree: "/d/bae-worktrees/side-2".into(),
            branch: "work".into(),
            brief: "b.md".into(),
            since: stamp(at),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
}

fn changes_of(db: &Connection, id: Id) -> Vec<Change> {
    read(db).item(id).unwrap().changes.clone()
}

#[test]
fn an_edit_of_the_details_records_the_details_before_and_after() {
    let db = db();
    let id = item(&db, vec![]);
    let before = read(&db).item(id).unwrap().details.clone();
    edit_details(&db, id, "new details", NOON + 5);
    assert_eq!(
        changes_of(&db, id),
        vec![Change {
            id: 1,
            made: stamp(NOON + 5),
            what: What::Details {
                old: before,
                new: "new details".into()
            },
            read: None,
        }]
    );
}

#[test]
fn an_edit_that_leaves_the_details_as_they_are_records_nothing() {
    let db = db();
    let id = item(&db, vec![]);
    let same = read(&db).item(id).unwrap().details.clone();
    edit_details(&db, id, &same, NOON + 5);
    assert!(changes_of(&db, id).is_empty());
}

#[test]
fn only_changes_since_the_take_that_nobody_read_are_unread() {
    let db = db();
    let id = item(&db, vec![]);
    edit_details(&db, id, "before the take", NOON + 1);
    assert!(read(&db).item(id).unwrap().unread_changes().is_empty());
    take(&db, id, NOON + 2);
    assert!(read(&db).item(id).unwrap().unread_changes().is_empty());
    edit_details(&db, id, "after the take", NOON + 3);
    edit_details(&db, id, "after the take, again", NOON + 4);
    let lists = read(&db);
    let unread: Vec<i64> = lists
        .item(id)
        .unwrap()
        .unread_changes()
        .iter()
        .map(|change| change.id)
        .collect();
    assert_eq!(unread, vec![2, 3]);
    mark_read(&db, &[2], &stamp(NOON + 5)).unwrap();
    let lists = read(&db);
    let item = lists.item(id).unwrap();
    assert_eq!(
        item.unread_changes()
            .iter()
            .map(|change| change.id)
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(item.changes[1].read, Some(stamp(NOON + 5)));
}

#[test]
fn a_change_of_an_item_that_does_not_run_is_never_unread() {
    let db = db();
    let id = item(&db, vec![]);
    edit_details(&db, id, "edited", NOON + 3);
    assert!(read(&db).item(id).unwrap().unread_changes().is_empty());
}

#[test]
fn words_print_the_details_before_and_after_whole() {
    let db = db();
    let id = item(&db, vec![]);
    edit_details(&db, id, "first line\nsecond line\n", NOON + 5);
    let lists = read(&db);
    let said = words(&lists, &lists.item(id).unwrap().changes[0]);
    assert!(said.contains("--- details before ---"), "{said}");
    assert!(
        said.contains("--- details after ---\nfirst line\nsecond line\n"),
        "{said}"
    );
}

#[test]
fn words_print_a_decisions_title_and_body_whole() {
    let db = db();
    let id = item(&db, vec![]);
    let asked = question(&db);
    let decision = decision_answering(&db, asked);
    record_decision(&db, id, decision, &stamp(NOON + 5)).unwrap();
    let lists = read(&db);
    let said = words(&lists, &lists.item(id).unwrap().changes[0]);
    assert!(said.contains("Chose a way\nwhy\n"), "{said}");
}
