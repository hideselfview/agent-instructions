use super::*;
use crate::shots_cache::{Key, stage};
use crate::testing::Scratch;
use serde_json::Value;

const LANES: [Lane; 2] = [Lane::MAC, Lane::ANDROID];
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// Keeps a run of the demo repo's COMMIT on <lane> under <prefix>, by this
/// builds' shots version, whose report holds <findings> as its
/// findings.json and a board picture and shot of each of <pictures>.
fn cache_run(
    places: &Places,
    lane: Lane,
    prefix: &str,
    findings: Value,
    pictures: &[&str],
) -> Cached {
    let staging = stage(
        places,
        &Key {
            repo: "demo".to_string(),
            commit: COMMIT.to_string(),
            lane,
            environment: "26B131-17C529".to_string(),
            prefix: prefix.to_string(),
            versions: crate::shots_program::Versions {
                capture: VERSIONS.capture.clone(),

                comparison: VERSIONS.comparison.clone(),
            },
        },
    )
    .unwrap();
    let report = staging.report();
    std::fs::create_dir_all(report.join("img")).unwrap();
    std::fs::write(report.join("findings.json"), findings.to_string()).unwrap();
    for scene in pictures {
        for side in ["board", "shot"] {
            std::fs::write(
                report.join(format!("img/{scene}.{side}.png")),
                format!("{scene} {side}"),
            )
            .unwrap();
        }
    }
    staging.finish(1, "pairs fail the run\n").unwrap()
}

/// The findings.json of a run of every mac component that checked the
/// MacCheckbox row `off`, with one finding and one warning, and none of the
/// row `on`, whose shot no capture file declares.
fn checkbox_findings() -> Value {
    json!({
        "version": VERSIONS.comparison,
        "capture_version": VERSIONS.capture,
        "commit": COMMIT,
        "scenes": ["mac.component.checkbox.off", "mac.component.row.plain"],
        "sizes": {
            "mac.component.checkbox.off": {
                "board": {"width": 110.5, "height": 16.0},
                "shot": {"width": 396.0, "height": 40.0},
            },
            "mac.component.row.plain": {
                "board": {"width": 300.0, "height": 28.0},
                "shot": {"width": 300.0, "height": 28.0},
            },
        },
        "components": [
            {"name": "MacCheckbox", "platform": "mac",
             "rows": ["mac.component.checkbox.off", "mac.component.checkbox.on"]},
            {"name": "MacRow", "platform": "mac", "rows": ["mac.component.row.plain"]},
        ],
        "findings": [
            {"scene": "mac.component.checkbox.off", "level": "finding", "kind": "role-differs",
             "component": "MacCheckbox", "element": {"role": "text", "words": "Sync"},
             "shot_role": "toggle", "sentence": "Sync is a text on the board and a toggle in the shot"},
            {"scene": "mac.component.checkbox.off", "level": "warning", "kind": "frame-offset",
             "dx": 1.0, "dy": 0.0, "sentence": "The shot's frame is 1 pt right of the board's"},
            {"scene": "mac.component.row.plain", "level": "finding", "kind": "missing",
             "element": {"role": "text", "words": "Title"}, "sentence": "Shot is missing the text Title"},
        ],
    })
}

fn no_capture(_: Lane, _: &str) -> Option<RunPage> {
    None
}

#[test]
fn a_component_s_rows_show_the_cached_run_s_pairs_and_findings_in_the_preview_s_order() {
    let scratch = Scratch::new("component-shots-rows");
    let places = scratch.places();
    cache_run(
        &places,
        Lane::MAC,
        "mac.component.",
        checkbox_findings(),
        &["mac.component.checkbox.off"],
    );
    let shots = shots(&places, "demo", COMMIT, "MacCheckbox", &LANES, &no_capture).unwrap();
    let json = serde_json::to_value(&shots).unwrap();
    let run = &json["platforms"][0]["run"];
    assert_eq!(json["platforms"].as_array().unwrap().len(), 1, "{json}");
    assert_eq!(json["platforms"][0]["lane"], "mac");
    assert_eq!(json["platforms"][0]["prefix"], "mac.component.checkbox.");
    assert_eq!(run["prefix"], "mac.component.");
    assert_eq!(
        run["rows"],
        json!([
            {"shot": "mac.component.checkbox.off", "state": "off",
             "sizes": {"board": {"width": 110.5, "height": 16.0},
                       "shot": {"width": 396.0, "height": 40.0}},
             "findings": [
                {"level": "finding", "sentence": "Sync is a text on the board and a toggle in the shot"},
                {"level": "warning", "sentence": "The shot's frame is 1 pt right of the board's"},
             ]},
            {"shot": "mac.component.checkbox.on", "state": "on", "sizes": null, "findings": []},
        ])
    );
    assert_eq!(json["platforms"][0]["capture"], Value::Null);
}

#[test]
fn a_component_both_platforms_share_has_a_platform_for_each_lane() {
    let scratch = Scratch::new("component-shots-shared");
    let places = scratch.places();
    let shots = shots(&places, "demo", COMMIT, "Artwork", &LANES, &no_capture).unwrap();
    let lanes: Vec<(&str, &str)> = shots
        .platforms
        .iter()
        .map(|p| (p.lane.as_str(), p.prefix.as_str()))
        .collect();
    assert_eq!(
        lanes,
        vec![
            ("mac", "mac.component.artwork."),
            ("android", "android.component.artwork.")
        ]
    );
    assert!(shots.platforms.iter().all(|p| p.run.is_none()));
}

#[test]
fn a_run_of_another_commit_or_shots_version_shows_no_run() {
    let scratch = Scratch::new("component-shots-other");
    let places = scratch.places();
    cache_run(
        &places,
        Lane::MAC,
        "mac.component.checkbox.",
        checkbox_findings(),
        &[],
    );
    let other = "f".repeat(40);
    let shots = shots(&places, "demo", &other, "MacCheckbox", &LANES, &no_capture).unwrap();
    assert!(shots.platforms[0].run.is_none());
}

#[test]
fn a_shots_run_shows_while_it_runs_and_once_it_ended_for_the_page_s_commit_only() {
    let scratch = Scratch::new("component-shots-capture");
    let places = scratch.places();
    let page = |reference: &str, phase| RunPage {
        lane: "mac".to_string(),
        reference: Some(reference.to_string()),
        phase,
        pid: 7,
        started_at: 100,
        ended_at: None,
        exit_code: None,
        signal: None,
        output: "waiting for a mac app checkout\n".to_string(),
        output_cut: false,
        output_stale: None,
    };
    let other = "f".repeat(40);
    let shown = |reference: &str, phase| {
        let capture = |_: Lane, _: &str| Some(page(reference, phase));
        shots(&places, "demo", COMMIT, "MacCheckbox", &LANES, &capture)
            .unwrap()
            .platforms[0]
            .capture
            .is_some()
    };
    assert!(shown(COMMIT, Phase::Running));
    assert!(shown(COMMIT, Phase::Failed));
    assert!(
        shown(&other, Phase::Running),
        "a run of another commit still holds the prefix"
    );
    assert!(!shown(&other, Phase::Failed));
}

#[test]
fn pruning_keeps_an_old_run_until_the_dashboard_finishes_reading_it() {
    let scratch = Scratch::new("component-shots-lease");
    let tool = scratch.tool(0);
    let places = &tool.places;
    let prefix = "mac.component.checkbox.";
    let cached = cache_run(places, Lane::MAC, prefix, checkbox_findings(), &[]);
    let run = &cached.folder;
    crate::testing::age(&run.join(shots_cache::RUN_FILE), 30);

    let prune = || shots_cache::prune_runs(&tool, crate::prune::Run::Real, 0);
    let read = shots_cache::read_covering(
        places,
        "demo",
        COMMIT,
        Lane::MAC,
        prefix,
        &VERSIONS,
        |cached| {
            prune()?;
            read_run(cached, "MacCheckbox", Lane::MAC, prefix).map(Some)
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(read.rows.len(), 2);
    prune().unwrap();
    assert!(run.exists(), "the dashboard renewed the run's last use");

    crate::testing::age(&run.join(shots_cache::RUN_FILE), 30);
    prune().unwrap();
    assert!(!run.exists());
}

#[test]
fn a_cached_run_s_board_and_shot_pictures_are_read_by_scene() {
    let scratch = Scratch::new("component-shots-pictures");
    let places = scratch.places();
    cache_run(
        &places,
        Lane::MAC,
        "mac.component.",
        checkbox_findings(),
        &["mac.component.checkbox.off"],
    );
    let read = |scene: &str, side| picture(&places, "demo", COMMIT, Lane::MAC, scene, side);
    assert_eq!(
        read("mac.component.checkbox.off", Side::Board).unwrap(),
        Some(b"mac.component.checkbox.off board".to_vec())
    );
    assert_eq!(
        read("mac.component.checkbox.off", Side::Shot).unwrap(),
        Some(b"mac.component.checkbox.off shot".to_vec())
    );
    assert_eq!(read("mac.component.checkbox.on", Side::Shot).unwrap(), None);
    assert_eq!(
        read("android.component.checkbox.on", Side::Shot).unwrap(),
        None
    );
    assert!(read("mac.component/../../x", Side::Shot).is_err());
}

#[test]
fn a_name_that_is_no_component_s_is_refused() {
    let scratch = Scratch::new("component-shots-name");
    let places = scratch.places();
    for name in ["row", "../MacRow", ""] {
        assert!(
            shots(&places, "demo", COMMIT, name, &LANES, &no_capture).is_err(),
            "{name:?}"
        );
    }
}

/// A repo whose origin/main holds the preview of MacCheckbox; returns it and
/// origin/main's commit.
fn checkbox_repo(scratch: &Scratch) -> (std::path::PathBuf, String) {
    let dir = scratch.dir.join("demo");
    crate::testing::repo(&dir, "");
    let preview = dir.join(preview_path("MacCheckbox"));
    std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
    std::fs::write(
        &preview,
        "<!-- @dsCard height=40 -->\n<html><head></head></html>",
    )
    .unwrap();
    crate::testing::git(&dir, &["add", "design"]);
    crate::testing::git(&dir, &["commit", "-q", "-m", "checkbox"]);
    let commit = crate::lists::design_paths::full_sha(&dir, "HEAD").unwrap();
    crate::testing::git(&dir, &["update-ref", "refs/remotes/origin/main", &commit]);
    (dir, commit)
}

#[test]
fn the_page_names_the_component_and_the_commit_origin_main_names() {
    let scratch = Scratch::new("component-shots-page");
    let places = scratch.places();
    let (repo, commit) = checkbox_repo(&scratch);
    let page = page(&places, "demo", "demo", &repo, "MacCheckbox", true).unwrap();
    let start = page.find("id=\"data\">").unwrap() + "id=\"data\">".len();
    let end = start + page[start..].find("</script>").unwrap();
    let data: Value = serde_json::from_str(&page[start..end]).unwrap();
    assert_eq!(
        data,
        json!({
            "project": "demo",
            "component": "MacCheckbox",
            "commit": commit,
            "here": true,
            "designs": "/designs/demo",
            "base": format!("/designs/demo/{commit}"),
        })
    );
    let error = super::page(&places, "demo", "demo", &repo, "MacRow", true)
        .unwrap_err()
        .text();
    assert!(
        error.contains("design/system/components/MacRow/preview.html"),
        "{error}"
    );
}

#[test]
fn a_cached_run_that_lists_no_rows_of_the_component_fails_the_read() {
    let scratch = Scratch::new("component-shots-unlisted");
    let places = scratch.places();
    let mut findings = checkbox_findings();
    findings["components"] = json!([]);
    cache_run(&places, Lane::MAC, "mac.component.", findings, &[]);
    let error = shots(&places, "demo", COMMIT, "MacCheckbox", &LANES, &no_capture)
        .unwrap_err()
        .text();
    assert!(
        error.contains("lists no preview rows of MacCheckbox on mac"),
        "{error}"
    );
}

#[test]
fn a_listed_row_whose_shot_is_not_the_component_s_fails_the_read() {
    let scratch = Scratch::new("component-shots-stray-row");
    let places = scratch.places();
    let mut findings = checkbox_findings();
    findings["components"][0]["rows"] = json!(["mac.component.row.plain"]);
    cache_run(&places, Lane::MAC, "mac.component.", findings, &[]);
    let error = shots(&places, "demo", COMMIT, "MacCheckbox", &LANES, &no_capture)
        .unwrap_err()
        .text();
    assert!(
        error.contains("mac.component.row.plain") && error.contains("mac.component.checkbox."),
        "{error}"
    );
}
