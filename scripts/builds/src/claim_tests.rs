use super::*;

/// The claim file and the JSON build a note the same way: a state with no
/// note is refused by both, and a known state gives its variant.
#[test]
fn a_state_without_a_note_is_refused_by_the_file_and_the_json() {
    assert_eq!(Note::from_fields(None, None).unwrap(), None);
    assert_eq!(
        Note::from_fields(Some("why".into()), Some("held".into())).unwrap(),
        Some(Note::Held("why".into()))
    );
    assert!(Note::from_fields(None, Some("held".into())).is_err());
    let json = r#"{"who":"a","since":"2026-11-18 10:00","note":null,"note_state":"held"}"#;
    assert!(serde_json::from_str::<Claim>(json).is_err());
}
