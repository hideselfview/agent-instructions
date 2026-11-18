use super::*;
use crate::lists::fixtures::db;

#[test]
fn a_check_replaces_the_one_before_it() {
    let db = db();
    assert_eq!(read(&db).unwrap(), None);
    record(&db, 100, Some("fetching failed")).unwrap();
    assert_eq!(
        read(&db).unwrap(),
        Some((100, Some("fetching failed".into())))
    );
    record(&db, 200, None).unwrap();
    assert_eq!(read(&db).unwrap(), Some((200, None)));
}

#[test]
fn the_table_refuses_an_empty_fault() {
    let db = db();
    assert!(record(&db, 100, Some("  ")).is_err());
}
