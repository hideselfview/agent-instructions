use super::*;
use crate::testing::Scratch;

#[test]
fn the_schema_applies_to_an_empty_database_and_every_table_is_strict() {
    let conn = memory();
    let tables: Vec<(String, i64)> = conn
        .prepare("SELECT name, strict FROM pragma_table_list WHERE schema = 'main' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(tables.len() > 30, "{tables:?}");
    for (name, strict) in tables {
        assert_eq!(strict, 1, "{name} is not STRICT");
    }
    let violations = conn.prepare("PRAGMA foreign_key_check").unwrap().query_map([], |_| Ok(())).unwrap().count();
    assert_eq!(violations, 0);
}

#[test]
fn a_new_file_gets_the_whole_schema_the_stamp_and_wal_in_its_first_transaction() {
    let scratch = Scratch::new("db-new");
    let path = scratch.dir.join(DATABASE);
    let conn = open(&path, Make::Allowed).unwrap();
    assert_eq!(stamp_of(&conn, &path).unwrap(), stamp());
    assert!(stamp() > HOOK_ONLY_STAMP);
    let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0)).unwrap();
    assert_eq!(mode, "wal");
    let sync: i64 = conn.pragma_query_value(None, "synchronous", |row| row.get(0)).unwrap();
    assert_eq!(sync, 2, "FULL");
    let keys: i64 = conn.pragma_query_value(None, "foreign_keys", |row| row.get(0)).unwrap();
    assert_eq!(keys, 1);
    assert_eq!(schema_rows(&conn).unwrap(), schema_rows(&memory()).unwrap());
    // Opened again, it is the same database; read-only, too.
    drop(conn);
    open(&path, Make::Never).unwrap();
    assert!(open_reader(&path).unwrap().is_some());
}

#[test]
fn a_file_that_is_not_there_is_not_made_by_a_reader_or_a_writer_that_may_not() {
    let scratch = Scratch::new("db-missing");
    let path = scratch.dir.join(DATABASE);
    assert!(open(&path, Make::Never).is_err());
    assert!(open_reader(&path).unwrap().is_none());
    assert!(!path.exists());
}

#[test]
fn a_stamp_of_another_schema_and_tables_without_a_stamp_are_refused() {
    let scratch = Scratch::new("db-stamp");
    let path = scratch.dir.join(DATABASE);
    open(&path, Make::Allowed).unwrap().pragma_update(None, "user_version", 12345).unwrap();
    assert!(open(&path, Make::Allowed).unwrap_err().text().contains("another builds schema"));
    assert!(open_reader(&path).unwrap_err().text().contains("another builds schema"));
    let bare = scratch.dir.join("bare.sqlite");
    Connection::open(&bare).unwrap().execute_batch("CREATE TABLE t (x)").unwrap();
    assert!(open(&bare, Make::Allowed).unwrap_err().text().contains("no schema stamp"));
}

#[test]
fn the_stamp_follows_the_schema_text() {
    let a = stamp();
    assert_eq!(a, stamp());
    assert!((2..i64::from(i32::MAX)).contains(&a));
}

#[test]
fn a_busy_store_waits_for_the_other_write_to_end_and_names_nothing_to_time_out() {
    let scratch = Scratch::new("db-busy");
    let path = scratch.dir.join(DATABASE);
    let first = open(&path, Make::Allowed).unwrap();
    first.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let second = {
        let path = path.clone();
        std::thread::spawn(move || {
            let conn = open(&path, Make::Never).unwrap();
            tx.send(()).unwrap();
            conn.execute_batch("BEGIN IMMEDIATE").unwrap();
            conn.execute_batch("COMMIT").unwrap();
        })
    };
    rx.recv().unwrap();
    first.execute_batch("COMMIT").unwrap();
    second.join().unwrap();
}
