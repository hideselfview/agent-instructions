use super::*;
use crate::compare::normalize::Entry;
use crate::compare::{Aspect, Finding, Part, Warning};
use crate::names::Name;
use crate::shots::tests::LIST;
use crate::shots::{Bounds, ElementList, Role};
use crate::verify::Checked;

fn entry(words: &str, kind: Role) -> Entry {
    Entry {
        index: 2,
        folded: Vec::new(),
        kind,
        words: words.to_string(),
        value: String::new(),
        enabled: true,
        selected: false,
    }
}

/// A checked pair whose board draws its element 2 with the component MacRow.
fn checked(name: &str, findings: Vec<Finding>, warnings: Vec<Warning>) -> Checked {
    let mut board = ElementList::parse(LIST).unwrap();
    board.elements[2].component = Some("MacRow".to_string());
    Checked {
        name: Name::parse(name).unwrap(),
        findings,
        warnings,
        against: None,
        board,
        shot: ElementList::parse(LIST).unwrap(),
        pairs: Vec::new(),
        offset: (0.0, 0.0),
        board_hash: "h".to_string(),
        board_png: "board.png".into(),
        shot_png: "shot.png".into(),
    }
}

#[test]
fn each_finding_and_warning_is_a_record_with_its_kind_component_element_and_numbers() {
    let mut on = entry("Sync", Role::Toggle);
    on.value = "on".to_string();
    let mut off = on.clone();
    off.value = "off".to_string();
    let checked = vec![
        checked(
            "mac.welcome.a",
            vec![
                Finding::Missing(entry("Set up", Role::Button)),
                Finding::RoleDiffers {
                    board: entry("Done", Role::Text),
                    shot: entry("Done", Role::Button),
                },
                Finding::StateDiffers {
                    board: on,
                    shot: off,
                    aspect: Aspect::Value,
                },
            ],
            vec![
                Warning::FrameOffset {
                    dx: 8.04,
                    dy: -7.96,
                },
                Warning::Colour {
                    entry: entry("", Role::Image),
                    part: Part::Background,
                    board: [0x22, 0x22, 0x30],
                    shot: [0x1E, 0x1E, 0x1E],
                    delta: 4.25,
                },
            ],
        ),
        checked(
            "mac.welcome.b",
            Vec::new(),
            vec![
                Warning::Moved {
                    entry: entry("Library", Role::Text),
                    dx: 40.08,
                    dy: 0.0,
                },
                Warning::Region {
                    bounds: Bounds {
                        x: 16.0,
                        y: 32.0,
                        width: 80.0,
                        height: 16.0,
                    },
                    delta: 9.0,
                },
            ],
        ),
    ];
    let json: serde_json::Value = serde_json::from_str(&text("abc123", None, &checked)).unwrap();
    let expected = serde_json::json!({
        "version": crate::VERSION,
        "commit": "abc123",
        "scenes": ["mac.welcome.a", "mac.welcome.b"],
        "boards": {"mac.welcome.a": "h", "mac.welcome.b": "h"},
        "findings": [
            {"scene": "mac.welcome.a", "level": "finding", "kind": "missing", "component": "MacRow",
             "element": {"role": "button", "words": "Set up"},
             "sentence": "Shot is missing the button “Set up”"},
            {"scene": "mac.welcome.a", "level": "finding", "kind": "role-differs", "component": "MacRow",
             "element": {"role": "text", "words": "Done"}, "shot_role": "button",
             "sentence": "“Done” is a text on the board and a button in the shot"},
            {"scene": "mac.welcome.a", "level": "finding", "kind": "state-differs", "component": "MacRow",
             "element": {"role": "toggle", "words": "Sync"}, "aspect": "value",
             "board": "on", "shot": "off",
             "sentence": "The toggle “Sync” has the value “on” on the board and “off” in the shot"},
            {"scene": "mac.welcome.a", "level": "warning", "kind": "frame-offset", "dx": 8.0, "dy": -8.0,
             "sentence": "The shot's frame is 8 pt right and 8 pt above of the board's"},
            {"scene": "mac.welcome.a", "level": "warning", "kind": "colour", "component": "MacRow",
             "element": {"role": "image", "words": ""}, "part": "background",
             "board": "#222230", "shot": "#1E1E1E", "delta": 4.25,
             "sentence": "The background of the unlabelled image is #222230 on the board and #1E1E1E in the shot"},
            {"scene": "mac.welcome.b", "level": "warning", "kind": "moved", "component": "MacRow",
             "element": {"role": "text", "words": "Library"}, "dx": 40.1, "dy": 0.0,
             "sentence": "“Library” moved 40.1 pt right in the shot"},
            {"scene": "mac.welcome.b", "level": "warning", "kind": "region",
             "x": 16.0, "y": 32.0, "width": 80.0, "height": 16.0, "delta": 9.0,
             "sentence": "The pictures differ in the area 80 pt wide and 16 pt high at (16, 32)"},
        ],
    });
    assert_eq!(json, expected);
}

#[test]
fn the_file_is_written_beside_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let checked = vec![checked(
        "mac.welcome.a",
        vec![Finding::Extra(entry("Beta", Role::Text))],
        Vec::new(),
    )];
    write(dir.path(), "abc123", Some("tree1"), &checked).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(json["findings"][0]["kind"], "extra");
    assert_eq!(
        json["findings"][0].get("component"),
        None,
        "an element only the shot has has no component"
    );
    assert_eq!(json["scenes"], serde_json::json!(["mac.welcome.a"]));
    assert_eq!(json["app_tree"], "tree1");
}

#[test]
fn a_later_run_compares_with_the_file_as_its_before_run() {
    let beta = Finding::Extra(entry("Beta", Role::Text));
    let checked = vec![checked(
        "mac.welcome.a",
        vec![beta.clone()],
        vec![Warning::FrameOffset { dx: 1.0, dy: 0.0 }],
    )];
    let before = crate::before::Before::parse(
        &text("abc123", None, &checked),
        "findings.json",
        crate::VERSION,
    )
    .unwrap();
    assert_eq!(before.commit(), "abc123");
    let against = |findings: &[Finding]| {
        let sentences: Vec<String> = findings.iter().map(Finding::sentence).collect();
        before.against("mac.welcome.a", &sentences)
    };
    assert_eq!(against(&[beta]), crate::before::Against::Held);
    assert_eq!(
        against(&[Finding::Missing(entry("Set up", Role::Button))]),
        crate::before::Against::Worse
    );
}
