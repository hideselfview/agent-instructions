use super::*;
use crate::lists::fixtures::{NOON, db, draft, read, stamp};

#[test]
fn a_release_makes_one_item_and_a_second_try_makes_none() {
    let db = db();
    let first = queue_once(
        &db,
        "1.96.0",
        &draft("Move the Rust pin to 1.96.0"),
        &stamp(NOON),
    )
    .unwrap()
    .expect("the first try makes the item");
    let again = queue_once(
        &db,
        "1.96.0",
        &draft("Move the Rust pin again to 1.96.0"),
        &stamp(NOON + 1),
    )
    .unwrap();
    assert_eq!(again, None);
    assert_eq!(queued(&db, "1.96.0").unwrap(), Some(first));
    assert_eq!(read(&db).items.len(), 1);
}

#[test]
fn another_release_makes_another_item() {
    let db = db();
    queue_once(
        &db,
        "1.96.0",
        &draft("Move the Rust pin to 1.96.0"),
        &stamp(NOON),
    )
    .unwrap();
    let next = queue_once(
        &db,
        "1.97.0",
        &draft("Move the Rust pin to 1.97.0"),
        &stamp(NOON + 1),
    )
    .unwrap();
    assert!(next.is_some());
    assert_eq!(read(&db).items.len(), 2);
}

#[test]
fn the_table_refuses_a_second_row_for_a_version_and_a_name_that_is_no_version() {
    let db = db();
    let id = queue_once(
        &db,
        "1.96.0",
        &draft("Move the Rust pin to 1.96.0"),
        &stamp(NOON),
    )
    .unwrap()
    .unwrap();
    let twice = db.execute(
        "INSERT INTO rust_release_item (version, item_id) VALUES ('1.96.0', ?1)",
        [id],
    );
    assert!(twice.is_err());
    let named = db.execute(
        "INSERT INTO rust_release_item (version, item_id) VALUES ('stable', ?1)",
        [id],
    );
    assert!(named.is_err());
}
