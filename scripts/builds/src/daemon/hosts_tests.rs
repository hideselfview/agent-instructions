use super::*;
use crate::cargo_runs::{Start, begin, end};
use crate::db;
use crate::host::tests::fake_ssh;
use crate::testing::Scratch;

fn start(host: &str, at: i64, command: &str) -> Start {
    Start {
        started_at: at,
        repo: "demo".to_string(),
        checkout: "side-1".to_string(),
        host: host.to_string(),
        pid: std::process::id(),
        command: command.to_string(),
    }
}

#[test]
fn a_host_row_says_what_the_host_is_running_and_a_host_that_does_not_answer_says_why() {
    let scratch = Scratch::new("daemon-hosts");
    let places = scratch.places();
    std::fs::create_dir_all(&places.home).unwrap();
    let file = host::hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 user@m1 10\ngone unreachable 4\n").unwrap();
    let ssh = fake_ssh(&scratch.dir.join("bin"), &scratch.dir.join("remote-home"));
    let database = places.lists.join("demo-project").join(DATABASE);
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    db::open(&database, db::Make::Allowed).unwrap();
    let now = 10_000;
    let finished = begin(&database, &start("local", now - 100, "check -p demo")).unwrap();
    end(&database, finished, now - 90, 0).unwrap();
    begin(&database, &start("m1", now - 20, "test -p demo")).unwrap();
    begin(&database, &start("m1", now - 5, "clippy --workspace")).unwrap();

    let rows = read(&places, &ssh, now).unwrap();
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["local", "m1", "gone"]);
    assert!(rows[0].running.is_empty());
    assert_eq!(rows[0].last_hour, 1);
    assert!(rows[0].room.is_some() && rows[0].target.is_none());
    let commands: Vec<&str> = rows[1].running.iter().map(|r| r.command.as_str()).collect();
    assert_eq!(commands, ["test -p demo", "clippy --workspace"]);
    assert_eq!(rows[1].last_hour, 2);
    assert_eq!(rows[1].running[0].checkout, "side-1");
    assert!(rows[2].room.is_none());
    assert!(
        rows[2]
            .unreachable
            .as_deref()
            .is_some_and(|said| said.contains("Connection refused")),
        "{:?}",
        rows[2].unreachable
    );
}

#[test]
fn no_store_means_no_runs() {
    let scratch = Scratch::new("daemon-hosts-empty");
    let places = scratch.places();
    let ssh = fake_ssh(&scratch.dir.join("bin"), &scratch.dir.join("remote-home"));
    let rows = read(&places, &ssh, 5).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].running.is_empty());
}
