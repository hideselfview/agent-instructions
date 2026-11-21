use super::*;

const MISSING: &str = "Shot is missing the button “Set up”";
const EXTRA: &str = "Shot has the text “Beta”, which the board does not";

/// A before run of version v1 at commit abc that checked mac.a.b, with one
/// finding and one warning, and mac.a.c, with none.
fn before() -> Before {
    let text = serde_json::json!({
        "version": "v1",
        "commit": "abc",
        "scenes": ["mac.a.b", "mac.a.c"],
        "sizes": {},
        "components": [],
        "findings": [
            {"scene": "mac.a.b", "level": "finding", "kind": "missing",
             "element": {"role": "button", "words": "Set up"}, "sentence": MISSING},
            {"scene": "mac.a.b", "level": "warning", "kind": "moved",
             "element": {"role": "text", "words": "Beta"}, "dx": 1.0, "dy": 0.0,
             "sentence": EXTRA},
        ],
    })
    .to_string();
    Before::parse(&text, "before.json", "v1").unwrap()
}

fn sentences(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|t| t.to_string()).collect()
}

#[test]
fn a_scene_the_before_run_did_not_check_is_new() {
    assert_eq!(before().against("mac.a.d", &sentences(&[])), Against::New);
}

#[test]
fn a_pair_with_only_the_before_runs_findings_holds() {
    let before = before();
    assert_eq!(
        before.against("mac.a.b", &sentences(&[MISSING])),
        Against::Held
    );
    assert_eq!(before.against("mac.a.b", &sentences(&[])), Against::Held);
    assert_eq!(before.against("mac.a.c", &sentences(&[])), Against::Held);
}

#[test]
fn a_pair_with_a_finding_the_before_run_lacks_is_worse() {
    let before = before();
    assert_eq!(
        before.against("mac.a.c", &sentences(&[MISSING])),
        Against::Worse
    );
    assert_eq!(
        before.against("mac.a.b", &sentences(&[MISSING, MISSING])),
        Against::Worse,
        "each sentence counts as often as it appears"
    );
    assert_eq!(
        before.against("mac.a.b", &sentences(&[EXTRA])),
        Against::Worse,
        "a warning of the before run is no finding"
    );
}

#[test]
fn a_file_of_another_version_is_refused() {
    let text = r#"{"version": "v2", "commit": "abc", "scenes": [], "sizes": {}, "components": [], "findings": []}"#;
    let error = Before::parse(text, "before.json", "v1").unwrap_err();
    assert!(error.contains("builds-shots version v2"), "{error}");
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
