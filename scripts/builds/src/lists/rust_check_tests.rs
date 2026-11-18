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

#[test]
fn a_record_that_can_not_be_read_is_a_finding_that_names_the_error() {
    use crate::lists::project::NewProject;
    use crate::lists::store::Store;
    use crate::lists::{Stamp, fixtures::NOON};
    let scratch = crate::testing::Scratch::new("rust-check-unreadable");
    let places = scratch.places();
    let store = Store::named(&places, "demo").unwrap();
    let stamp = Stamp {
        at: NOON,
        by: "test".into(),
    };
    store
        .create(
            &NewProject {
                name: "demo".into(),
                canvas: false,
                repos: vec!["bae".into()],
                places: Default::default(),
                sources: Default::default(),
                specs: Default::default(),
            },
            &stamp,
            "lists init",
        )
        .unwrap();
    assert_eq!(findings(&places), Vec::new());
    rusqlite::Connection::open(store.database())
        .unwrap()
        .execute_batch("DROP TABLE rust_check")
        .unwrap();
    let found = findings(&places);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0]
            .text
            .contains("could not read project demo's last check")
            && found[0].text.contains("rust_check"),
        "{}",
        found[0].text
    );
}
