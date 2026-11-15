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

#[test]
fn a_run_is_running_until_it_ends() {
    let (_scratch, path) = store("cargo-runs");
    let id = begin(&path, &start("m1", 1_000, std::process::id())).unwrap();
    let running = running(&path).unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].host, "m1");
    assert_eq!(running[0].command, "test -p demo");
    end(&path, id, 1_030, 101).unwrap();
    assert!(super::running(&path).unwrap().is_empty());
    let conn = db::open_reader(&path).unwrap().unwrap();
    let (ended, code): (i64, i64) = conn
        .query_row("SELECT ended_at, exit_code FROM cargo_runs", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!((ended, code), (1_030, 101));
}

#[test]
fn a_run_whose_process_is_gone_is_not_running() {
    let (_scratch, path) = store("cargo-runs-dead");
    // A process that has exited and been waited for.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    begin(&path, &start("local", 1_000, pid)).unwrap();
    assert!(running(&path).unwrap().is_empty());
}

#[test]
fn counts_runs_by_host_and_deletes_old_finished_ones() {
    let (_scratch, path) = store("cargo-runs-count");
    let old = begin(&path, &start("local", 100, std::process::id())).unwrap();
    end(&path, old, 110, 0).unwrap();
    let a = begin(&path, &start("m1", 2_000_000, std::process::id())).unwrap();
    end(&path, a, 2_000_010, 0).unwrap();
    begin(&path, &start("m1", 2_000_020, std::process::id())).unwrap();
    begin(&path, &start("local", 2_000_030, std::process::id())).unwrap();
    assert_eq!(
        started_since(&path, 0).unwrap(),
        [("local".to_string(), 1), ("m1".to_string(), 2)],
        "the old finished run went with the next begin"
    );
    assert_eq!(
        started_since(&path, 2_000_025).unwrap(),
        [("local".to_string(), 1)]
    );
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
