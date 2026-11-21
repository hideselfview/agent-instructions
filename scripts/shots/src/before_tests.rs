use super::*;
use crate::compare::Aspect;
use crate::findings_file::Element;

const MISSING: &str = "Shot is missing the button “Set up”";
const EXTRA: &str = "Shot has the text “Beta”, which the board does not";

/// A before run of version v1 at commit abc that checked mac.a.b, with one
/// finding and one warning, and mac.a.c, with none.
fn before() -> Before {
    let text = serde_json::json!({
        "version": "v1",
        "commit": "abc",
        "scenes": ["mac.a.b", "mac.a.c"],
        "failed": [],
        "sizes": {},
        "components": [],
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

#[test]
fn a_value_that_is_no_number_counts_as_farther_than_a_number() {
    let state = |board: &str, shot: &str| What::StateDiffers {
        element: element("field", "Name"),
        aspect: Aspect::Value,
        board: board.to_string(),
        shot: shot.to_string(),
    };
    let text = serde_json::json!({
        "version": "v1", "commit": "abc", "scenes": ["s"], "failed": [], "sizes": {},
        "components": [],
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
fn a_file_of_another_version_is_refused() {
    let text = r#"{"version": "v2", "commit": "abc", "scenes": [], "failed": [], "sizes": {}, "components": [], "findings": []}"#;
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
