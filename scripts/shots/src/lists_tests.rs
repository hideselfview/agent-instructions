use super::*;
use crate::record::Entry;

fn name(text: &str) -> Name {
    Name::parse(text).unwrap()
}

fn entry(board: &str) -> Entry {
    Entry {
        board: board.to_string(),
        shot: "s".to_string(),
        findings: Vec::new(),
        warnings: Vec::new(),
    }
}

#[test]
fn a_ready_board_no_shot_is_declared_for_is_to_build() {
    let declared = [name("mac.welcome.a")];
    let ready = ["mac.welcome.a".to_string(), "mac.welcome.b".to_string()];
    let found = lists(&declared, &ready, &Record::default(), &BTreeMap::new(), &[]);
    assert_eq!(found.to_build, ["mac.welcome.b"]);
}

#[test]
fn a_shot_whose_board_changed_since_it_was_confirmed_is_to_update() {
    let declared = [
        name("mac.welcome.a"),
        name("mac.welcome.b"),
        name("mac.welcome.c"),
    ];
    let mut record = Record::default();
    record.set("mac.welcome.a", entry("old"));
    record.set("mac.welcome.b", entry("same"));
    let hashes = BTreeMap::from([
        (name("mac.welcome.a"), "new".to_string()),
        (name("mac.welcome.b"), "same".to_string()),
        (name("mac.welcome.c"), "fresh".to_string()),
    ]);
    let found = lists(&declared, &[], &record, &hashes, &[]);
    // c was never confirmed: it is new, not to update.
    assert_eq!(found.to_update, ["mac.welcome.a"]);
}

#[test]
fn a_shot_with_no_board_is_to_delete_and_a_record_entry_with_none_is_not_listed_here() {
    let problems = [
        Problem {
            name: "mac.welcome.x".into(),
            declared: true,
            sentence: "s".into(),
        },
        Problem {
            name: "mac.welcome.y".into(),
            declared: false,
            sentence: "s".into(),
        },
    ];
    let found = lists(&[], &[], &Record::default(), &BTreeMap::new(), &problems);
    assert_eq!(found.to_delete, ["mac.welcome.x"]);
}
