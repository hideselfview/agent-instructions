use super::*;
use crate::command::Command;

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn flag_of(line: &str) -> Result<Flag> {
    let words = words(line);
    let parsed = Command::parse(&words)?;
    Flag::of(&Args::of(parsed.command, parsed.rest)?)
}

fn read(project: &str) -> Flag {
    Flag::Read(project.to_string())
}

fn unread(project: &str) -> Flag {
    Flag::Unread(project.to_string())
}

#[test]
fn a_command_that_does_not_read_the_flag_leaves_it_unread() {
    assert_eq!(
        flag_of("queue demo --project two --lane mac").unwrap(),
        unread("two")
    );
    assert_eq!(
        flag_of("app-build demo --list --project two").unwrap(),
        unread("two")
    );
}

#[test]
fn a_command_that_reads_the_flag_reads_it() {
    for line in [
        "item show 3 --project two",
        "app-build demo main --project two",
        "shots picture demo main mac.a.one shot --project two",
        "checks stats --project two",
    ] {
        assert_eq!(flag_of(line).unwrap(), read("two"), "{line}");
    }
}

#[test]
fn the_words_of_a_command_that_builds_runs_hold_no_flag_of_its_own() {
    assert_eq!(
        flag_of("lock demo --lane mac --project two run --project theirs").unwrap(),
        unread("two")
    );
    assert_eq!(
        flag_of("lock demo --lane mac run --project theirs").unwrap(),
        Flag::Absent
    );
    assert_eq!(
        flag_of("remote run demo main -- run --project theirs").unwrap(),
        Flag::Absent
    );
    assert_eq!(
        flag_of("decision find --project p -- --project x").unwrap(),
        read("p")
    );
}

#[test]
fn a_flag_with_no_name_or_given_twice_is_refused() {
    for line in ["queue demo --project", "queue demo --project a --project b"] {
        assert!(flag_of(line).is_err(), "{line}");
    }
}
