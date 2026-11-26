use super::*;
use crate::lists::Lists;
use crate::names::Apps;
use crate::report::{Header, write};
use crate::shots::tests::{LIST, png};
use crate::verify::Outcome;

/// A report that `write` made of one pair whose shot lacks the board's
/// button: the board is `LIST` and the shot's list has no "Continue".
fn report_of_a_pair_missing_a_button(dir: &Path) -> std::path::PathBuf {
    report_of_a_pair(dir, true)
}

/// A report of one pair whose shot lacks the board's button when <missing>,
/// and is the board's own list otherwise.
fn report_of_a_pair(dir: &Path, missing: bool) -> std::path::PathBuf {
    let name = Name::parse("mac.welcome.a", &Apps::default()).unwrap();
    let pictures = dir.join("in");
    std::fs::create_dir_all(&pictures).unwrap();
    let board_list = ElementList::parse(LIST).unwrap();
    let mut shot_list = board_list.clone();
    if missing {
        shot_list.elements.pop();
    }
    std::fs::write(pictures.join("board.png"), png(800, 380)).unwrap();
    std::fs::write(pictures.join("shot.png"), png(800, 380)).unwrap();
    std::fs::write(pictures.join("board.json"), LIST).unwrap();
    std::fs::write(
        pictures.join("shot.json"),
        serde_json::to_string(&shot_list).unwrap(),
    )
    .unwrap();
    let pair = verify::compare_pair(
        &name,
        Drawn {
            list: board_list,
            picture: Picture::decode(&png(800, 380)).unwrap(),
            png: pictures.join("board.png"),
            hash: "b0a2".to_string(),
        },
        &Shot {
            list: shot_list,
            picture: Picture::decode(&png(800, 380)).unwrap(),
        },
        pictures.join("shot.png"),
    );
    assert_eq!(!pair.findings.is_empty(), missing);
    let outcome = Outcome {
        runtime: String::new(),
        checked: vec![pair],
        problems: Vec::new(),
        lists: Lists::default(),
        not_taken: Vec::new(),
        failed: Vec::new(),
        components: Default::default(),
    };
    let report = dir.join("report");
    write(
        &report,
        &outcome,
        &Header {
            capture_version: "capture".to_string(),
            version: "old".to_string(),
            commit: "abc123def".to_string(),
            base: None,
            runtime: String::new(),
            chrome: String::new(),
        },
    )
    .unwrap();
    report
}

fn records(text: &str) -> serde_json::Value {
    let file: serde_json::Value = serde_json::from_str(text).unwrap();
    file["findings"].clone()
}

#[test]
fn a_report_is_compared_again_to_the_findings_it_holds_under_the_new_version() {
    let dir = tempfile::tempdir().unwrap();
    let report = report_of_a_pair_missing_a_button(dir.path());
    let written = std::fs::read_to_string(report.join(FILE)).unwrap();
    let again = text(&report, "new").unwrap();
    assert_eq!(records(&again), records(&written));
    assert_ne!(records(&again), serde_json::json!([]));
    let file: serde_json::Value = serde_json::from_str(&again).unwrap();
    assert_eq!(file["version"], "new");
    assert_eq!(
        file["passed"], false,
        "a pair with a finding and no before run fails the run"
    );
    assert_eq!(
        file["capture_version"], "capture",
        "the pictures keep the capture version of the checker that made them"
    );
    assert_eq!(file["commit"], "abc123def");
    assert_eq!(
        file["boards"],
        serde_json::json!({"mac.welcome.a": "b0a2"}),
        "the pictures keep the hash of the board they were drawn from"
    );
    assert!(
        file.get("failed").is_none() && file.get("components").is_none(),
        "the recheck states nothing of the run's failed scenes or components"
    );
}

#[test]
fn a_report_without_element_lists_is_refused_naming_the_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let report = report_of_a_pair_missing_a_button(dir.path());
    std::fs::remove_file(report.join("img/mac.welcome.a.shot.json")).unwrap();
    let error = text(&report, "new").unwrap_err();
    assert!(error.contains("mac.welcome.a.shot.json"), "{error}");
}

#[test]
fn a_run_whose_report_names_problems_or_failed_captures_fails_though_its_pairs_hold() {
    let dir = tempfile::tempdir().unwrap();
    let report = report_of_a_pair(dir.path(), false);
    let passed = |text: &str| {
        let file: serde_json::Value = serde_json::from_str(text).unwrap();
        file["passed"].as_bool().unwrap()
    };
    assert!(passed(&text(&report, "new").unwrap()));
    let path = report.join(FILE);
    let original = std::fs::read_to_string(&path).unwrap();
    let mut file: serde_json::Value = serde_json::from_str(&original).unwrap();
    file["problems"] = serde_json::json!(1);
    std::fs::write(&path, file.to_string()).unwrap();
    assert!(
        !passed(&text(&report, "new").unwrap()),
        "a problem fails the run"
    );
    let mut file: serde_json::Value = serde_json::from_str(&original).unwrap();
    file["failed"] =
        serde_json::json!([{"scene": "mac.welcome.b", "error": "the capture crashed"}]);
    std::fs::write(&path, file.to_string()).unwrap();
    assert!(
        !passed(&text(&report, "new").unwrap()),
        "a scene the capture failed fails the run"
    );
}
