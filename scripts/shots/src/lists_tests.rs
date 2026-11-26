use super::*;
use crate::names::Apps;

fn name(text: &str) -> Name {
    Name::parse(text, &Apps::default()).unwrap()
}

#[test]
fn a_ready_board_no_shot_is_declared_for_is_to_build() {
    let declared = [name("mac.welcome.a")];
    let ready = ["mac.welcome.a".to_string(), "mac.welcome.b".to_string()];
    let found = lists(&declared, &ready, &[]);
    assert_eq!(found.to_build, ["mac.welcome.b"]);
}

#[test]
fn a_shot_with_no_board_is_to_delete() {
    let problems = [Problem {
        name: "mac.welcome.x".into(),
        sentence: "s".into(),
    }];
    let found = lists(&[], &[], &problems);
    assert_eq!(found.to_delete, ["mac.welcome.x"]);
}
