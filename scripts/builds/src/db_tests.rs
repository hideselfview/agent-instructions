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
    let violations = conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn a_new_file_gets_the_whole_schema_the_stamp_and_wal_in_its_first_transaction() {
    let scratch = Scratch::new("db-new");
    let path = scratch.dir.join(DATABASE);
    let conn = open(&path, Make::Allowed).unwrap();
    assert_eq!(stamp_of(&conn, &path).unwrap(), stamp());
    assert!(stamp() > HOOK_ONLY_STAMP);
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let sync: i64 = conn
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .unwrap();
    assert_eq!(sync, 2, "FULL");
    let keys: i64 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
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
    open(&path, Make::Allowed)
        .unwrap()
        .pragma_update(None, "user_version", 12345)
        .unwrap();
    assert!(
        open(&path, Make::Allowed)
            .unwrap_err()
            .text()
            .contains("another builds schema")
    );
    assert!(
        open_reader(&path)
            .unwrap_err()
            .text()
            .contains("another builds schema")
    );
    let bare = scratch.dir.join("bare.sqlite");
    Connection::open(&bare)
        .unwrap()
        .execute_batch("CREATE TABLE t (x)")
        .unwrap();
    assert!(
        open(&bare, Make::Allowed)
            .unwrap_err()
            .text()
            .contains("no schema stamp")
    );
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

/// Each table's owner files, from the SQL files that create them: the `.rs`
/// beside the `.sql`, and its `_tests.rs`.
fn table_owners(dir: &Path) -> Vec<(String, Vec<String>)> {
    let mut owners = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|e| e == "sql") {
                let stem = path.file_stem().unwrap().to_string_lossy().to_string();
                let beside = |suffix: &str| {
                    path.with_file_name(format!("{stem}{suffix}.rs"))
                        .strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                };
                let text = std::fs::read_to_string(&path).unwrap();
                let words: Vec<&str> = text
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .filter(|w| !w.is_empty())
                    .collect();
                for w in words.windows(3) {
                    if w[0] == "CREATE" && w[1] == "TABLE" {
                        owners.push((w[2].to_string(), vec![beside(""), beside("_tests")]));
                    }
                }
            }
        }
    }
    owners
}

/// The tables a source text writes: after `INSERT INTO`, `INSERT OR <x> INTO`,
/// `UPDATE` or `DELETE FROM`.
fn tables_written(source: &str) -> Vec<String> {
    let words: Vec<&str> = source
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    let mut written = Vec::new();
    for (at, word) in words.iter().enumerate() {
        let after = |n: usize| words.get(at + n).copied().unwrap_or("");
        let table = match *word {
            "INSERT" if after(1) == "INTO" => after(2),
            "INSERT" if after(1) == "OR" && after(3) == "INTO" => after(4),
            "UPDATE" => after(1),
            "DELETE" if after(1) == "FROM" => after(2),
            _ => continue,
        };
        written.push(table.to_string());
    }
    written
}

/// Every write in <sources> (path, text) to a table of <owners> from a file
/// that is not one of its owner files.
fn foreign_writes(sources: &[(String, String)], owners: &[(String, Vec<String>)]) -> Vec<String> {
    let mut found = Vec::new();
    for (path, text) in sources {
        for table in tables_written(text) {
            if let Some((_, files)) = owners.iter().find(|(name, _)| *name == table)
                && !files.contains(path)
            {
                found.push(format!("{path} writes {table}, which {} owns", files[0]));
            }
        }
    }
    found
}

fn rust_sources(dir: &Path) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                sources.push((name, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    sources.sort();
    sources
}

#[test]
fn a_write_to_a_table_outside_the_file_that_owns_it_is_found() {
    let owners = vec![(
        "area".to_string(),
        vec![
            "lists/area.rs".to_string(),
            "lists/area_tests.rs".to_string(),
        ],
    )];
    // The statements are built so this file does not hold them as text.
    let write = format!("{} INTO area (name) VALUES (?1)", "INSERT");
    let other = format!("{} OR IGNORE INTO area (name) VALUES (?1)", "INSERT");
    let delete = format!("{} FROM area", "DELETE");
    let update = format!("{} area SET name = ?1", "UPDATE");
    let source = |path: &str, text: &str| (path.to_string(), format!("db.execute(\"{text}\")"));
    let sources = vec![
        source("lists/area.rs", &write),
        source("lists/area_tests.rs", &delete),
        source("lists/item.rs", &write),
        source("lists/item_tests.rs", &other),
        source("lists/store.rs", &delete),
        source("lists/project.rs", &update),
        source("lists/project.rs", "SELECT name FROM area"),
    ];
    assert_eq!(
        foreign_writes(&sources, &owners),
        vec![
            "lists/item.rs writes area, which lists/area.rs owns",
            "lists/item_tests.rs writes area, which lists/area.rs owns",
            "lists/store.rs writes area, which lists/area.rs owns",
            "lists/project.rs writes area, which lists/area.rs owns",
        ]
    );
}

#[test]
fn every_table_is_written_only_by_the_file_that_owns_it() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let owners = table_owners(&src);
    assert!(owners.len() > 30, "{owners:?}");
    for (table, files) in &owners {
        for file in files {
            if !file.ends_with("_tests.rs") {
                assert!(src.join(file).exists(), "{table}'s owner {file} is missing");
            }
        }
    }
    let found = foreign_writes(&rust_sources(&src), &owners);
    assert!(found.is_empty(), "{}", found.join("\n"));
}
