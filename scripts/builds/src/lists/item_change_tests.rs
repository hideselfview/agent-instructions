use super::*;
use crate::lists::fixtures::{NOON, db, item, read, stamp};
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
