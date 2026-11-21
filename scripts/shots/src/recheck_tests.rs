use super::*;
use crate::lists::Lists;
use crate::report::{Header, write};
use crate::shots::tests::{LIST, png};
use crate::verify::Outcome;

/// A report that `write` made of one pair whose shot lacks the board's
/// button: the board is `LIST` and the shot's list has no "Continue".
fn report_of_a_pair_missing_a_button(dir: &Path) -> std::path::PathBuf {
    let name = Name::parse("mac.welcome.a").unwrap();
    let pictures = dir.join("in");
    std::fs::create_dir_all(&pictures).unwrap();
    let board_list = ElementList::parse(LIST).unwrap();
    let mut shot_list = board_list.clone();
    shot_list.elements.pop();
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
        },
        &Shot {
            list: shot_list,
            picture: Picture::decode(&png(800, 380)).unwrap(),
        },
        pictures.join("shot.png"),
    );
    assert!(!pair.findings.is_empty(), "the shot lacks a board element");
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
    assert_eq!(file["commit"], "abc123def");
}

#[test]
fn a_report_without_element_lists_is_refused_naming_the_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let report = report_of_a_pair_missing_a_button(dir.path());
    std::fs::remove_file(report.join("img/mac.welcome.a.shot.json")).unwrap();
    let error = text(&report, "new").unwrap_err();
    assert!(error.contains("mac.welcome.a.shot.json"), "{error}");
}
