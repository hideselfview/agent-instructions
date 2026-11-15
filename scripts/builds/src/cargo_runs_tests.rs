use super::*;
use crate::testing::Scratch;

fn store(name: &str) -> (Scratch, std::path::PathBuf) {
    let scratch = Scratch::new(name);
    let dir = scratch.dir.join("store");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(db::DATABASE);
    db::open(&path, db::Make::Allowed).unwrap();
    (scratch, path)
}

fn start(host: &str, at: i64, pid: u32) -> Start {
    Start {
        started_at: at,
        repo: "demo".to_string(),
        checkout: "side-1".to_string(),
        host: host.to_string(),
        pid,
        command: "test -p demo".to_string(),
    }
}

fn rows(path: &std::path::Path) -> Vec<(i64, Option<i64>, Option<i64>)> {
    let conn = db::open_reader(path).unwrap().unwrap();
    let mut statement = conn
        .prepare("SELECT started_at, ended_at, exit_code FROM cargo_runs ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

#[test]
fn a_run_has_no_end_until_it_ends() {
    let (_scratch, path) = store("cargo-runs");
    let id = begin(&path, &start("m1", 1_000, std::process::id())).unwrap();
    assert_eq!(rows(&path), [(1_000, None, None)]);
    end(&path, id, 1_030, 101).unwrap();
    assert_eq!(rows(&path), [(1_000, Some(1_030), Some(101))]);
}

#[test]
fn the_next_begin_deletes_runs_that_ended_over_a_week_ago() {
    let (_scratch, path) = store("cargo-runs-count");
    let old = begin(&path, &start("local", 100, std::process::id())).unwrap();
    end(&path, old, 110, 0).unwrap();
    begin(&path, &start("m1", 2_000_000, std::process::id())).unwrap();
    assert_eq!(rows(&path), [(2_000_000, None, None)]);
}

#[test]
fn the_table_refuses_a_run_that_ends_without_a_status() {
    let (_scratch, path) = store("cargo-runs-check");
    let conn = db::open(&path, db::Make::Never).unwrap();
    let refused = conn.execute(
        "INSERT INTO cargo_runs (started_at, ended_at, repo, checkout, host, pid, command) VALUES (5, 6, 'r', 'c', 'h', 1, 'x')",
        [],
    );
    assert!(refused.is_err());
}
