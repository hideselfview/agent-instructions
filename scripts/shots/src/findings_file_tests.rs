use super::*;
use crate::compare::normalize::{Entry, enabled_state};
use crate::compare::{Aspect, Finding, Part, Warning};
use crate::component_files::{Component, Listing, Place, Row};
use crate::names::{Name, Platform};
use crate::shots::tests::LIST;
use crate::shots::{Bounds, ElementList, Role};
use crate::verify::Checked;

/// The checker version the tests write and read files of.
const VERSION: &str = "v1";
/// The capture version the tests write files under.
const CAPTURE: &str = "c1";

fn entry(words: &str, kind: Role) -> Entry {
    Entry {
        index: 2,
        folded: Vec::new(),
        kind,
        words: words.to_string(),
        value: String::new(),
        enabled: enabled_state(kind, true),
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
                Warning::WindowCorners,
            ],
        ),
    ];
    let json: serde_json::Value = serde_json::from_str(&text(
        VERSION,
        CAPTURE,
        "abc123",
        &checked,
        &[],
        &Listing::default(),
        0,
    ))
    .unwrap();
    // The elements of the checked() pairs' board and shot, which are alike.
    let seen_elements = serde_json::json!([
        {"role": "text", "words": "Enter code"},
        {"role": "field", "words": "Restore code or invite"},
        {"role": "button", "words": "Continue"},
    ]);
    let seen = serde_json::json!({"board": seen_elements, "shot": seen_elements});
    let expected = serde_json::json!({
        "version": VERSION,
        "capture_version": CAPTURE,
        "commit": "abc123",
        "scenes": ["mac.welcome.a", "mac.welcome.b"],
        "failed": [],
        "problems": 0,
        "seen": {
            "mac.welcome.a": seen,
            "mac.welcome.b": seen,
        },
        "sizes": {
            "mac.welcome.a": {"board": checked[0].board.size, "shot": checked[0].shot.size},
            "mac.welcome.b": {"board": checked[1].board.size, "shot": checked[1].shot.size},
        },
        "components": [],
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
            {"scene": "mac.welcome.b", "level": "warning", "kind": "window-corners",
             "sentence": "The board and shot show different window corner shapes"},
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
    let outcome = Outcome {
        runtime: String::new(),
        checked,
        problems: Vec::new(),
        lists: Default::default(),
        not_taken: Vec::new(),
        failed: Vec::new(),
        components: Listing::default(),
    };
    write(dir.path(), VERSION, CAPTURE, "abc123", &outcome).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(json["findings"][0]["kind"], "extra");
    assert_eq!(
        json["findings"][0].get("component"),
        None,
        "an element only the shot has has no component"
    );
    assert_eq!(json["scenes"], serde_json::json!(["mac.welcome.a"]));
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
        &text(
            VERSION,
            CAPTURE,
            "abc123",
            &checked,
            &[],
            &Listing::default(),
            0,
        ),
        "findings.json",
        VERSION,
    )
    .unwrap();
    assert_eq!(before.commit(), "abc123");
    let against = |findings: &[Finding]| {
        let whats: Vec<crate::findings_file::What> = findings
            .iter()
            .map(crate::findings_file::What::from)
            .collect();
        before.against("mac.welcome.a", &whats)
    };
    assert_eq!(against(&[beta]), crate::before::Against::Held);
    assert_eq!(
        against(&[Finding::Missing(entry("Continue", Role::Button))]),
        crate::before::Against::Worse,
        "the base compared Continue and had no finding on it"
    );
    assert_eq!(
        against(&[Finding::Missing(entry("Set up", Role::Button))]),
        crate::before::Against::Held,
        "neither side of the base's pair held Set up, so its finding is a first observation"
    );
}

#[test]
fn the_file_names_each_component_s_preview_rows_by_their_shots_in_the_preview_s_order() {
    let row = |name: &str, place| Row {
        shot: Name::parse(name).unwrap(),
        place,
    };
    let listing = Listing {
        components: vec![Component {
            name: "MacCheckbox".to_string(),
            platform: Platform::Mac,
            file: "baeShots/Components/MacCheckbox.swift".into(),
            file_exists: false,
            rows: vec![
                row(
                    "mac.component.checkbox.on",
                    Place::Elsewhere("baeShots/Settings.swift".into()),
                ),
                row("mac.component.checkbox.off", Place::Nowhere),
            ],
        }],
        strays: Vec::new(),
        unnamed: Vec::new(),
        design_only: 0,
        design_only_rows: 0,
    };
    let json: serde_json::Value =
        serde_json::from_str(&text(VERSION, CAPTURE, "abc123", &[], &[], &listing, 0)).unwrap();
    assert_eq!(
        json["components"],
        serde_json::json!([{
            "name": "MacCheckbox",
            "platform": "mac",
            "rows": ["mac.component.checkbox.on", "mac.component.checkbox.off"],
        }])
    );
}

#[test]
fn the_file_gives_each_pair_s_board_and_shot_sizes_in_points() {
    let mut pair = checked("mac.welcome.a", Vec::new(), Vec::new());
    pair.board.size.width = 110.5;
    pair.board.size.height = 16.0;
    pair.shot.size.width = 396.0;
    pair.shot.size.height = 40.0;
    let json: serde_json::Value = serde_json::from_str(&text(
        VERSION,
        CAPTURE,
        "abc123",
        &[pair],
        &[],
        &Listing::default(),
        0,
    ))
    .unwrap();
    assert_eq!(
        json["sizes"],
        serde_json::json!({"mac.welcome.a": {
            "board": {"width": 110.5, "height": 16.0},
            "shot": {"width": 396.0, "height": 40.0},
        }})
    );
}

#[test]
fn a_pair_the_base_failed_to_capture_is_unrendered_against_it_and_one_it_never_selected_is_new() {
    let failed = vec![crate::capture::Failed {
        name: Name::parse("mac.welcome.crashed").unwrap(),
        why: "the capture crashed".to_string(),
    }];
    let before = crate::before::Before::parse(
        &text(
            VERSION,
            CAPTURE,
            "abc123",
            &[],
            &failed,
            &Listing::default(),
            0,
        ),
        "findings.json",
        VERSION,
    )
    .unwrap();
    let missing = [crate::findings_file::What::from(&Finding::Missing(entry(
        "Set up",
        Role::Button,
    )))];
    assert_eq!(
        before.against("mac.welcome.crashed", &missing),
        crate::before::Against::Unrendered
    );
    assert_eq!(
        before.against("mac.welcome.added", &missing),
        crate::before::Against::New
    );
}
