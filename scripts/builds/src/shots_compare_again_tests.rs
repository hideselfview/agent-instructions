use super::*;
use crate::shots_program::{VERSIONS, Versions};
use crate::testing::Scratch;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const ENVIRONMENT: &str = "26B131-17C529";

fn key(capture: &str, comparison: &str) -> Key {
    in_environment(ENVIRONMENT, capture, comparison)
}

fn in_environment(environment: &str, capture: &str, comparison: &str) -> Key {
    Key::new(
        "demo",
        COMMIT,
        Lane::Mac,
        environment,
        "mac.",
        &Versions {
            capture: capture.to_string(),
            comparison: comparison.to_string(),
        },
    )
}

/// Caches a run of <key> whose report holds a page, a board picture, a shot
/// picture and a findings.json that names the key's versions and one finding.
fn cache_run(places: &Places, key: &Key) -> Cached {
    cache_run_at(places, key, crate::app_timing::now())
}

fn cache_run_at(places: &Places, key: &Key, recorded_at: i64) -> Cached {
    let staging = shots_cache::stage(places, key).unwrap();
    let report = staging.report();
    std::fs::create_dir_all(report.join("img")).unwrap();
    std::fs::write(report.join("index.html"), "<html>old findings</html>").unwrap();
    std::fs::write(report.join("img/mac.a.board.png"), "board").unwrap();
    std::fs::write(report.join("img/mac.a.shot.png"), "shot").unwrap();
    std::fs::write(
        report.join("findings.json"),
        serde_json::json!({
            "version": key.versions.comparison,
            "capture_version": key.versions.capture,
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
    staging.finish_at(1, "old text\n", recorded_at).unwrap()
}

/// A program that records each call in the file `calls` and prints the
/// findings `builds shots recheck` of <comparison> makes of a run of
/// <capture>'s pictures: no finding, where the cached run had one.
fn recheck_program(scratch: &Scratch, capture: &str, comparison: &str, passed: bool) -> PathBuf {
    let program = scratch.dir.join("program.sh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\necho \"$@\" >> \"{}\"\necho '{{\"version\": \"{comparison}\", \"capture_version\": \"{capture}\", \"commit\": \"c\", \"passed\": {passed}, \"scenes\": [\"mac.a\"], \"seen\": {{\"mac.a\": {{\"board\": [{{\"role\": \"text\", \"words\": \"Beta\"}}], \"shot\": []}}}}, \"findings\": [{{\"scene\": \"mac.a\", \"level\": \"warning\", \"kind\": \"moved\", \"element\": {{\"role\": \"text\", \"words\": \"Beta\"}}, \"dx\": 1.0, \"dy\": 0.0, \"sentence\": \"new sentence\"}}]}}'\n",
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
    let capture = VERSIONS.capture.as_str();
    let old = cache_run(&places, &key(capture, "old-comparison"));
    let program = recheck_program(&scratch, capture, &VERSIONS.comparison, false);
    let new = key(capture, &VERSIONS.comparison);
    assert!(shots_cache::lookup(&places, &new).unwrap().is_none());
    let again = lookup(&places, &new, &program)
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
    assert_eq!(file["version"], VERSIONS.comparison.as_str());
    assert_eq!(file["capture_version"], capture);
    assert_eq!(file["findings"][0]["sentence"], "new sentence");
    assert_eq!(file["findings"].as_array().unwrap().len(), 1);
    assert!(file["sizes"]["mac.a"].is_object(), "{file}");
    assert_eq!(
        file["seen"]["mac.a"]["board"][0]["words"], "Beta",
        "the elements the comparison read come from the new comparison, not the old file, \
         which may hold none: {file}"
    );
    assert!(
        again.findings.contains("compared again"),
        "{}",
        again.findings
    );
    assert_eq!(
        again.exit_code, 1,
        "the exit status is the checker's verdict on the new findings, not the old run's"
    );
}

#[test]
fn the_run_a_branch_compares_with_is_made_by_comparing_the_base_s_cached_run_again() {
    let scratch = Scratch::new("compare-again-before");
    let places = scratch.places();
    let capture = VERSIONS.capture.as_str();
    let old = cache_run(&places, &key(capture, "old-comparison"));
    let program = recheck_program(&scratch, capture, &VERSIONS.comparison, true);
    let made = before_run(
        &places,
        "demo",
        COMMIT,
        Lane::Mac,
        ENVIRONMENT,
        "mac.a.",
        &program,
    )
    .unwrap()
    .expect("the base's run of another comparison version is compared again");
    assert_eq!(made.key.versions, *VERSIONS);
    assert_eq!(made.key.prefix, old.key.prefix);
    assert_eq!(made.exit_code, 0);
    assert!(made.findings_json().is_file());
    let before = std::fs::read_to_string(made.findings_json()).unwrap();
    assert!(
        before.contains("\"sizes\""),
        "it keeps what a before file needs"
    );
}

#[test]
fn a_comparison_that_prints_no_elements_read_makes_no_run() {
    let scratch = Scratch::new("compare-again-no-seen");
    let places = scratch.places();
    let capture = VERSIONS.capture.as_str();
    cache_run(&places, &key(capture, "old-comparison"));
    let program = scratch.dir.join("program.sh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\necho '{{\"version\": \"{}\", \"capture_version\": \"{capture}\", \"commit\": \"c\", \"passed\": true, \"scenes\": [\"mac.a\"], \"findings\": []}}'\n",
            VERSIONS.comparison
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let new = key(capture, &VERSIONS.comparison);
    let error = lookup(&places, &new, &program).unwrap_err().text();
    assert!(error.contains("`seen`"), "{error}");
    assert!(shots_cache::lookup(&places, &new).unwrap().is_none());
}

#[test]
fn a_change_to_the_capture_compares_nothing_again() {
    let scratch = Scratch::new("compare-again-capture");
    let places = scratch.places();
    cache_run(&places, &key("capture-1", "comparison-1"));
    let program = recheck_program(&scratch, "capture-2", "comparison-1", true);
    let new = key("capture-2", "comparison-1");
    assert!(compare_again(&places, &new, &program).unwrap().is_none());
    assert_eq!(calls(&scratch), "", "no program ran");
    assert!(shots_cache::lookup(&places, &new).unwrap().is_none());
}

#[test]
fn a_run_whose_findings_a_program_of_other_versions_made_is_refused() {
    let scratch = Scratch::new("compare-again-mismatch");
    let places = scratch.places();
    cache_run(&places, &key("capture-1", "comparison-1"));
    let program = recheck_program(&scratch, "capture-1", "comparison-3", true);
    let new = key("capture-1", "comparison-2");
    let error = compare_again(&places, &new, &program).unwrap_err().text();
    assert!(error.contains("comparison-3"), "{error}");
    assert!(shots_cache::lookup(&places, &new).unwrap().is_none());
}

#[test]
fn only_a_run_of_the_same_environment_is_compared_again() {
    let scratch = Scratch::new("compare-again-environment");
    let places = scratch.places();
    let capture = VERSIONS.capture.as_str();
    cache_run(
        &places,
        &in_environment("25G241-27B52", capture, "old-comparison"),
    );
    let program = recheck_program(&scratch, capture, &VERSIONS.comparison, true);
    let new = key(capture, &VERSIONS.comparison);
    assert!(
        compare_again(&places, &new, &program).unwrap().is_none(),
        "the run of another environment is no source"
    );
    assert!(
        before_run(
            &places,
            "demo",
            COMMIT,
            Lane::Mac,
            ENVIRONMENT,
            "mac.a.",
            &program
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(calls(&scratch), "", "no program ran");
    let own = cache_run(&places, &key(capture, "old-comparison"));
    let made = before_run(
        &places,
        "demo",
        COMMIT,
        Lane::Mac,
        ENVIRONMENT,
        "mac.a.",
        &program,
    )
    .unwrap()
    .expect("the run of its own environment is compared again");
    assert_eq!(made.key.environment, ENVIRONMENT);
    assert!(calls(&scratch).contains(&own.report().display().to_string()));
}

#[test]
fn before_run_considers_a_covering_run_published_during_recheck() {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;

    let scratch = Scratch::new("compare-again-concurrent");
    let places = scratch.places();
    let capture = VERSIONS.capture.as_str();
    cache_run(&places, &key(capture, "old-comparison"));
    let recheck = recheck_program(&scratch, capture, &VERSIONS.comparison, true);
    let concurrent_key = Key {
        prefix: "mac.a.".into(),
        ..key(capture, &VERSIONS.comparison)
    };
    let started = scratch.dir.join("started");
    let resume = scratch.dir.join("resume");
    crate::testing::fifo(&started);
    crate::testing::fifo(&resume);
    let program = scratch.script(
        "wait-for-publication",
        &format!(
            r#"printf x > '{}' && IFS= read -r release < '{}' && exec '{}' "$@""#,
            started.display(),
            resume.display(),
            recheck.display(),
        ),
    );
    // Open both ends before starting recheck, so each open in its program
    // has a peer and the release byte can be sent even on failure.
    let mut fifo = std::fs::OpenOptions::new();
    fifo.read(true).write(true);
    let mut starts = fifo.open(started).unwrap();
    let mut resumes = fifo.open(resume).unwrap();
    let found = std::thread::scope(|scope| {
        let checking = scope.spawn(|| {
            before_run(
                &places,
                "demo",
                COMMIT,
                Lane::Mac,
                ENVIRONMENT,
                "mac.a.",
                &program,
            )
        });
        let published = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut ready = libc::pollfd {
                fd: starts.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            assert_eq!(
                unsafe { libc::poll(&mut ready, 1, 5000) },
                1,
                "recheck did not start"
            );
            starts.read_exact(&mut [0]).unwrap();
            // Record the other writer's later completion time through the
            // cache's normal publication path.
            cache_run_at(&places, &concurrent_key, i64::MAX);
        }));
        resumes.write_all(b"published\n").unwrap();
        let result = checking.join().unwrap();
        if let Err(panic) = published {
            std::panic::resume_unwind(panic);
        }
        result.unwrap().unwrap()
    });

    assert_eq!(found.key, concurrent_key);
}

#[test]
fn the_newest_run_of_a_commit_and_prefix_is_found_in_any_environment() {
    let scratch = Scratch::new("compare-again-newest");
    let places = scratch.places();
    let capture = VERSIONS.capture.as_str();
    cache_run(
        &places,
        &in_environment("25G241-27B52", capture, &VERSIONS.comparison),
    );
    let program = recheck_program(&scratch, capture, &VERSIONS.comparison, true);
    let found = lookup_newest(&places, "demo", COMMIT, Lane::Mac, "mac.", &program)
        .unwrap()
        .expect("a run of another environment is a commit's own run");
    assert_eq!(found.key.environment, "25G241-27B52");
    assert!(
        lookup_newest(&places, "demo", COMMIT, Lane::Mac, "mac.b.", &program)
            .unwrap()
            .is_none()
    );
}
