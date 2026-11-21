use super::*;
use crate::shots_program::Versions;
use crate::testing::Scratch;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn key(capture: &str, comparison: &str) -> Key {
    Key::new(
        "demo",
        COMMIT,
        Lane::Mac,
        "mac.",
        Versions {
            capture,
            comparison,
        },
    )
}

/// Caches a run of <key> whose report holds a page, a board picture, a shot
/// picture and a findings.json that names the key's versions and one finding.
fn cache_run(places: &Places, key: &Key) -> Cached {
    let staging = shots_cache::stage(places, key).unwrap();
    let report = staging.report();
    std::fs::create_dir_all(report.join("img")).unwrap();
    std::fs::write(report.join("index.html"), "<html>old findings</html>").unwrap();
    std::fs::write(report.join("img/mac.a.board.png"), "board").unwrap();
    std::fs::write(report.join("img/mac.a.shot.png"), "shot").unwrap();
    std::fs::write(
        report.join("findings.json"),
        serde_json::json!({
            "version": key.comparison_version,
            "capture_version": key.capture_version,
            "commit": "c",
            "scenes": ["mac.a"],
            "failed": [],
            "sizes": {"mac.a": {"board": {"width": 1.0, "height": 1.0}}},
            "components": [],
            "findings": [{"scene": "mac.a", "level": "finding", "kind": "missing",
                "sentence": "old sentence"}],
        })
        .to_string(),
    )
    .unwrap();
    staging.finish(1, "old text\n").unwrap()
}

/// A program that records each call in the file `calls` and prints the
/// findings `builds shots recheck` of <comparison> makes of a run of
/// <capture>'s pictures: no finding, where the cached run had one.
fn recheck_program(scratch: &Scratch, capture: &str, comparison: &str) -> PathBuf {
    let program = scratch.dir.join("program.sh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\necho \"$@\" >> \"{}\"\necho '{{\"version\": \"{comparison}\", \"capture_version\": \"{capture}\", \"commit\": \"c\", \"scenes\": [\"mac.a\"], \"findings\": [{{\"scene\": \"mac.a\", \"level\": \"warning\", \"kind\": \"moved\", \"element\": {{\"role\": \"text\", \"words\": \"Beta\"}}, \"dx\": 1.0, \"dy\": 0.0, \"sentence\": \"new sentence\"}}]}}'\n",
            scratch.dir.join("calls").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

fn calls(scratch: &Scratch) -> String {
    std::fs::read_to_string(scratch.dir.join("calls")).unwrap_or_default()
}

#[test]
fn a_change_to_the_comparison_alone_compares_the_cached_pictures_again_and_takes_no_shot() {
    let scratch = Scratch::new("compare-again");
    let places = scratch.places();
    let old = cache_run(&places, &key("capture-1", "comparison-1"));
    let program = recheck_program(&scratch, "capture-1", "comparison-2");
    let new = key("capture-1", "comparison-2");
    assert!(shots_cache::lookup(&places, &new).unwrap().is_none());
    let again = compare_again_with(&places, &new, &program)
        .unwrap()
        .expect("the run of the other comparison version is the source");
    assert!(
        calls(&scratch).contains(&format!(
            "shots recheck --report {}",
            old.report().display()
        )),
        "the program compared the cached report: {}",
        calls(&scratch)
    );
    // The run is cached under the new comparison version, beside the old one.
    let found = shots_cache::lookup(&places, &new).unwrap().expect("cached");
    assert_eq!(found.folder, again.folder);
    assert!(old.folder.exists());
    assert_ne!(old.folder, again.folder);
    // Its pictures are the cached run's, shared as links; its page is not.
    let (from, to) = (
        old.report().join("img/mac.a.shot.png"),
        again.report().join("img/mac.a.shot.png"),
    );
    assert_eq!(std::fs::read_to_string(&to).unwrap(), "shot");
    assert_eq!(
        std::fs::metadata(&from).unwrap().ino(),
        std::fs::metadata(&to).unwrap().ino()
    );
    assert!(!again.report().join("index.html").exists());
    // Its findings are the program's, over what the old file recorded of the
    // pictures.
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(again.findings_json()).unwrap()).unwrap();
    assert_eq!(file["version"], "comparison-2");
    assert_eq!(file["capture_version"], "capture-1");
    assert_eq!(file["findings"][0]["sentence"], "new sentence");
    assert_eq!(file["findings"].as_array().unwrap().len(), 1);
    assert!(file["sizes"]["mac.a"].is_object(), "{file}");
    assert!(
        again.findings.contains("compared again"),
        "{}",
        again.findings
    );
    assert_eq!(
        again.exit_code, 0,
        "the new findings hold only a warning, whatever the old run exited"
    );
    // The cached run is found by the lookup that serves a branch's run and the
    // one that finds its base run.
    let served = shots_cache::before_run(
        &places,
        "demo",
        COMMIT,
        Lane::Mac,
        "mac.a.",
        Versions {
            capture: "capture-1",
            comparison: "comparison-2",
        },
    )
    .unwrap()
    .expect("the run covers its prefix");
    assert_eq!(served.folder, again.folder);
}

#[test]
fn a_change_to_the_capture_compares_nothing_again() {
    let scratch = Scratch::new("compare-again-capture");
    let places = scratch.places();
    cache_run(&places, &key("capture-1", "comparison-1"));
    let program = recheck_program(&scratch, "capture-2", "comparison-1");
    let new = key("capture-2", "comparison-1");
    assert!(
        compare_again_with(&places, &new, &program)
            .unwrap()
            .is_none()
    );
    assert_eq!(calls(&scratch), "", "no program ran");
    assert!(shots_cache::lookup(&places, &new).unwrap().is_none());
}

#[test]
fn a_run_whose_findings_a_program_of_other_versions_made_is_refused() {
    let scratch = Scratch::new("compare-again-mismatch");
    let places = scratch.places();
    cache_run(&places, &key("capture-1", "comparison-1"));
    let program = recheck_program(&scratch, "capture-1", "comparison-3");
    let new = key("capture-1", "comparison-2");
    let error = compare_again_with(&places, &new, &program)
        .unwrap_err()
        .text();
    assert!(error.contains("comparison-3"), "{error}");
    assert!(shots_cache::lookup(&places, &new).unwrap().is_none());
}
