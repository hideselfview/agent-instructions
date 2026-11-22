use super::*;
use crate::compare::Aspect;
use crate::findings_file::Element;
use crate::shots::Role;

const MISSING: &str = "Shot is missing the button “Set up”";
const EXTRA: &str = "Shot has the text “Beta”, which the board does not";

/// A before run of version v1 at commit abc that checked mac.a.b, with one
/// finding and one warning, and mac.a.c, with none.
fn before() -> Before {
    let text = serde_json::json!({
        "version": "v1",
        "capture_version": "c1",
        "commit": "abc",
        "scenes": ["mac.a.b", "mac.a.c"],
        "failed": [],
        "problems": 0,
        "sizes": {},
        "components": [],
        "seen": {
            "mac.a.b": {
                "board": [
                    {"role": "button", "words": "Set up"},
                    {"role": "slider", "words": "Position"},
                    {"role": "slider", "words": "Volume"},
                    {"role": "text", "words": "Beta"},
                ],
                "shot": [
                    {"role": "slider", "words": "Position"},
                    {"role": "slider", "words": "Volume"},
                    {"role": "text", "words": "Beta"},
                ],
            },
            "mac.a.c": {
                "board": [{"role": "button", "words": "Set up"}],
                "shot": [{"role": "button", "words": "Set up"}],
            },
        },
        "findings": [
            {"scene": "mac.a.b", "level": "finding", "kind": "missing",
             "element": {"role": "button", "words": "Set up"}, "sentence": MISSING},
            {"scene": "mac.a.b", "level": "finding", "kind": "state-differs",
             "element": {"role": "slider", "words": "Position"}, "aspect": "value",
             "board": "35", "shot": "0.3366", "sentence": "x"},
            {"scene": "mac.a.b", "level": "warning", "kind": "moved",
             "element": {"role": "text", "words": "Beta"}, "dx": 1.0, "dy": 0.0,
             "sentence": EXTRA},
        ],
    })
    .to_string();
    Before::parse(&text, "before.json", "v1").unwrap()
}

fn element(role: &str, words: &str) -> Element {
    serde_json::from_value(serde_json::json!({"role": role, "words": words})).unwrap()
}

fn missing() -> What {
    What::Missing {
        element: element("button", "Set up"),
    }
}

fn slider(shot: &str) -> What {
    What::StateDiffers {
        element: element("slider", "Position"),
        aspect: Aspect::Value,
        board: "35".to_string(),
        shot: shot.to_string(),
    }
}

#[test]
fn a_scene_the_before_run_did_not_check_is_new() {
    assert_eq!(before().against("mac.a.d", &[]), Against::New);
}

#[test]
fn a_pair_with_only_the_before_runs_findings_holds() {
    let before = before();
    assert_eq!(
        before.against("mac.a.b", &[missing(), slider("0.3366")]),
        Against::Held
    );
    assert_eq!(before.against("mac.a.b", &[]), Against::Held);
    assert_eq!(before.against("mac.a.c", &[]), Against::Held);
}

#[test]
fn a_pair_with_a_finding_the_before_run_lacks_is_worse() {
    let before = before();
    assert_eq!(before.against("mac.a.c", &[missing()]), Against::Worse);
    assert_eq!(
        before.against("mac.a.b", &[missing(), missing(), slider("0.3366")]),
        Against::Worse,
        "each before finding covers one finding"
    );
    let extra = What::Extra {
        element: element("text", "Beta"),
    };
    assert_eq!(
        before.against("mac.a.b", &[extra]),
        Against::Worse,
        "a warning of the before run is no finding"
    );
}

#[test]
fn a_value_closer_to_the_board_is_no_new_finding() {
    assert_eq!(
        before().against("mac.a.b", &[missing(), slider("34")]),
        Against::Held,
        "board 35, base 0.3366, branch 34"
    );
}

#[test]
fn a_value_farther_from_the_board_is_worse() {
    assert_eq!(
        before().against("mac.a.b", &[missing(), slider("100")]),
        Against::Worse
    );
}

#[test]
fn a_finding_of_another_element_or_aspect_is_worse() {
    let other = What::StateDiffers {
        element: element("slider", "Volume"),
        aspect: Aspect::Value,
        board: "35".to_string(),
        shot: "34".to_string(),
    };
    assert_eq!(
        before().against("mac.a.b", &[missing(), other]),
        Against::Worse
    );
    let enabled = What::StateDiffers {
        element: element("slider", "Position"),
        aspect: Aspect::Enabled,
        board: "on".to_string(),
        shot: "off".to_string(),
    };
    assert_eq!(
        before().against("mac.a.b", &[missing(), enabled]),
        Against::Worse
    );
}

/// The finding of a state on a button the before run could not compare.
fn state_of_set_up(aspect: Aspect, board: &str, shot: &str) -> What {
    What::StateDiffers {
        element: element("button", "Set up"),
        aspect,
        board: board.to_string(),
        shot: shot.to_string(),
    }
}

#[test]
fn a_finding_on_an_element_the_before_run_reported_missing_is_a_first_observation() {
    // mac.a.b's before run holds `Set up` as missing from the shot. On the
    // branch the shot has the button, and it differs in its state.
    let before = before();
    assert_eq!(
        before.against(
            "mac.a.b",
            &[
                state_of_set_up(Aspect::Enabled, "enabled", "disabled"),
                slider("0.3366")
            ]
        ),
        Against::Held
    );
    let role = What::RoleDiffers {
        element: element("button", "Set up"),
        shot_role: Role::Text,
    };
    let out_of_order = What::OutOfOrder {
        element: element("button", "Set up"),
    };
    assert_eq!(
        before.against("mac.a.b", &[role, out_of_order, slider("0.3366")]),
        Against::Held
    );
}

#[test]
fn a_finding_on_an_element_the_before_run_reported_extra_is_a_first_observation() {
    let text = serde_json::json!({
        "version": "v1", "capture_version": "c1", "commit": "abc", "scenes": ["s"], "failed": [], "problems": 0, "sizes": {},
        "components": [], "seen": {},
        "findings": [{"scene": "s", "level": "finding", "kind": "extra",
            "element": {"role": "text", "words": "Beta"}, "sentence": EXTRA}],
    })
    .to_string();
    let before = Before::parse(&text, "before.json", "v1").unwrap();
    let state = What::StateDiffers {
        element: element("text", "Beta"),
        aspect: Aspect::Value,
        board: "a".to_string(),
        shot: "b".to_string(),
    };
    assert_eq!(before.against("s", &[state]), Against::Held);
}

/// A text element as a record names it.
fn text(words: &str) -> Element {
    element("text", words)
}

#[test]
fn a_finding_on_a_row_the_before_run_never_had_in_view_is_a_first_observation() {
    // The before run's board and shot hold no row `7`: its shot ended above
    // it. The branch reaches it, so the row's findings show for the first
    // time, whichever side lacks it.
    let before = before();
    let new_row = [
        What::Missing { element: text("7") },
        What::Extra {
            element: text("7 Weather Report Card 3:34"),
        },
        What::OutOfOrder { element: text("7") },
    ];
    let mut findings = vec![missing(), slider("0.3366")];
    findings.extend(new_row);
    assert_eq!(before.against("mac.a.b", &findings), Against::Held);
}

#[test]
fn a_finding_on_an_element_the_before_run_compared_is_still_worse_beside_new_rows() {
    // `Beta` is on the before run's board, so a missing `Beta` is new.
    let before = before();
    let findings = [
        missing(),
        slider("0.3366"),
        What::Missing { element: text("7") },
        What::Missing {
            element: text("Beta"),
        },
    ];
    assert_eq!(before.against("mac.a.b", &findings), Against::Worse);
    // An extra element the before run's shot held is new too.
    let extra = What::Extra {
        element: element("slider", "Volume"),
    };
    assert_eq!(
        before.against("mac.a.b", &[missing(), slider("0.3366"), extra]),
        Against::Worse
    );
}

#[test]
fn a_finding_on_an_element_the_before_run_matched_is_still_worse() {
    // The before run had no finding on `Position`'s enabled state, so the
    // element matched there: a new finding is a regression.
    let enabled = What::StateDiffers {
        element: element("slider", "Position"),
        aspect: Aspect::Enabled,
        board: "enabled".to_string(),
        shot: "disabled".to_string(),
    };
    assert_eq!(
        before().against("mac.a.b", &[missing(), slider("0.3366"), enabled]),
        Against::Worse
    );
    // `Set up` was missing, and a new missing or extra finding is no
    // newly matched element.
    assert_eq!(before().against("mac.a.c", &[missing()]), Against::Worse);
}

#[test]
fn a_first_observation_is_not_spent_by_one_finding() {
    // Two aspects of the newly matched button both show for the first time.
    assert_eq!(
        before().against(
            "mac.a.b",
            &[
                state_of_set_up(Aspect::Enabled, "enabled", "disabled"),
                state_of_set_up(Aspect::Selected, "selected", "not selected"),
                slider("0.3366"),
            ]
        ),
        Against::Held
    );
}

#[test]
fn a_value_that_is_no_number_counts_as_farther_than_a_number() {
    let state = |board: &str, shot: &str| What::StateDiffers {
        element: element("field", "Name"),
        aspect: Aspect::Value,
        board: board.to_string(),
        shot: shot.to_string(),
    };
    let text = serde_json::json!({
        "version": "v1", "capture_version": "c1", "commit": "abc", "scenes": ["s"], "failed": [], "problems": 0, "sizes": {},
        "components": [], "seen": {},
        "findings": [{"scene": "s", "level": "finding", "kind": "state-differs",
            "element": {"role": "field", "words": "Name"}, "aspect": "value",
            "board": "35", "shot": "34", "sentence": "x"}],
    })
    .to_string();
    let before = Before::parse(&text, "before.json", "v1").unwrap();
    assert_eq!(before.against("s", &[state("a", "c")]), Against::Held);
    assert_eq!(
        before.against("s", &[state("35", "10")]),
        Against::Held,
        "a field's text is no measured number, even when it parses as one"
    );
}

#[test]
fn an_empty_run_makes_every_scene_new() {
    let before = Before::empty("abc");
    assert_eq!(before.commit(), "abc");
    assert_eq!(before.against("mac.a.b", &[missing()]), Against::New);
    assert_eq!(before.against("mac.a.b", &[]), Against::New);
}

#[test]
fn a_file_of_another_version_is_refused() {
    let text = r#"{"version": "v2", "capture_version": "c1", "commit": "abc", "scenes": [], "failed": [], "problems": 0, "sizes": {}, "components": [], "seen": {}, "findings": []}"#;
    let error = Before::parse(text, "before.json", "v1").unwrap_err();
    assert!(error.contains("shots checker version v2"), "{error}");
    assert!(error.contains("this is version v1"), "{error}");
}

#[test]
fn a_file_that_is_no_findings_file_is_refused() {
    let error = Before::parse(r#"{"findings": []}"#, "before.json", "v1").unwrap_err();
    assert!(
        error.contains("before.json is not a shots run's findings.json"),
        "{error}"
    );
}

#[test]
fn the_before_run_names_its_commit() {
    assert_eq!(before().commit(), "abc");
}
