use super::*;
use crate::shots_program::Versions;
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
        Lane::MAC,
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
    cache_run_at(places, key, crate::app_timing::now(), 1)
}

fn cache_run_at(places: &Places, key: &Key, recorded_at: i64, exit_code: i64) -> Cached {
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
            "boards": {"mac.a": "h"},
            "failed": [],
            "sizes": {"mac.a": {"board": {"width": 1.0, "height": 1.0}}},
            "components": [],
            "findings": [{"scene": "mac.a", "level": "finding", "kind": "missing",
                "sentence": "old sentence"}],
        })
        .to_string(),
    )
    .unwrap();
    staging
        .finish_at(exit_code, "old text\n", recorded_at)
        .unwrap()
}

/// A program that records each call in the file `calls` and prints the
/// findings `builds shots recheck` of <comparison> makes of a run of
/// <capture>'s pictures: no finding, where the cached run had one.
fn recheck_program(scratch: &Scratch, capture: &str, comparison: &str, passed: bool) -> PathBuf {
    let program = scratch.dir.join("program.sh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\necho \"$@\" >> \"{}\"\necho '{{\"version\": \"{comparison}\", \"capture_version\": \"{capture}\", \"commit\": \"c\", \"passed\": {passed}, \"scenes\": [\"mac.a\"], \"boards\": {{\"mac.a\": \"h\"}}, \"seen\": {{\"mac.a\": {{\"board\": [{{\"role\": \"text\", \"words\": \"Beta\"}}], \"shot\": []}}}}, \"findings\": [{{\"scene\": \"mac.a\", \"level\": \"warning\", \"kind\": \"moved\", \"element\": {{\"role\": \"text\", \"words\": \"Beta\"}}, \"dx\": 1.0, \"dy\": 0.0, \"sentence\": \"new sentence\"}}]}}'\n",
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
    let capture = mac_versions().capture.as_str();
    let old = cache_run(&places, &key(capture, "old-comparison"));
    let program = recheck_program(&scratch, capture, &mac_versions().comparison, false);
    let new = key(capture, &mac_versions().comparison);
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
    assert_eq!(found.app_exit_code(), old.exit_code());
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
    assert_eq!(file["version"], mac_versions().comparison.as_str());
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
        again.exit_code(),
        1,
        "the exit status is the checker's verdict on the new findings, not the old run's"
    );
}

#[test]
fn the_run_a_branch_compares_with_is_made_by_comparing_the_base_s_cached_run_again() {
    let scratch = Scratch::new("compare-again-before");
    let places = scratch.places();
    let capture = mac_versions().capture.as_str();
    let old = cache_run(&places, &key(capture, "old-comparison"));
    let program = recheck_program(&scratch, capture, &mac_versions().comparison, true);
    let made = before_run(
        &places,
        "demo",
        COMMIT,
        Lane::MAC,
        ENVIRONMENT,
        "mac.a.",
        &program,
        &mut || Ok(Vec::new()),
    )
    .unwrap()
    .expect("the base's run of another comparison version is compared again");
    assert_eq!(made.key.versions, *mac_versions());
    assert_eq!(made.key.prefix, old.key.prefix);
    assert_eq!(made.exit_code(), 0);
    assert!(made.findings_json().is_file());
    let before = std::fs::read_to_string(made.findings_json()).unwrap();
    assert!(
        before.contains("\"sizes\""),
        "it keeps what a before file needs"
    );
}

#[test]
fn an_app_build_reuses_only_a_recheck_whose_source_and_build_record_passed() {
    use crate::app_timing::{self, BuildRecords, RunRecord, StepKind, StepResult, Timer};
    let old = key(&mac_versions().capture, "old-comparison");
    for exit_code in [0, 1] {
        let scratch = Scratch::new("reuse-recheck");
        let places = scratch.places();
        let program = recheck_program(
            &scratch,
            &mac_versions().capture,
            &mac_versions().comparison,
            exit_code != 0,
        );
        cache_run_at(&places, &old, app_timing::now(), exit_code);
        let store = crate::lists::fixtures::make_store_named(&places, "demo-project", &["demo"]);
        let records = BuildRecords::open(&places, "demo").unwrap().unwrap();
        let serve = |recorded| {
            crate::app_build::serve_cached(
                &places,
                "demo",
                COMMIT,
                vec![crate::app_build::all_shots(Lane::MAC)],
                &program,
                recorded,
                &mut crate::shots_declared::ShotDeclarations::new(&scratch.dir),
            )
            .unwrap()
        };
        assert_eq!(serve(Some(&records)).pending.len(), 1);
        app_timing::record(
            &store.database(),
            &RunRecord {
                started_at: app_timing::now(),
                repo: "demo".into(),
                lane: Lane::MAC,
                checkout: "side-1".into(),
                requested_commit: None,
                built_ref: "work".into(),
                built_commit: Some(COMMIT.to_string().into()),
                who: None,
                wait_ms: 0,
                outcome: crate::run_end::RunEnd::Passed,
                load: None,
                steps: vec![Timer::start().finish(
                    "shots mac.",
                    StepKind::Shots,
                    StepResult::Passed,
                )],
            },
        )
        .unwrap();
        assert_eq!(serve(Some(&records)).pending.is_empty(), exit_code == 0);
        assert_eq!(serve(None).cached_result().is_ok(), exit_code == 0);
    }
}

#[test]
fn a_comparison_that_prints_no_elements_read_makes_no_run() {
    let scratch = Scratch::new("compare-again-no-seen");
    let places = scratch.places();
    let capture = mac_versions().capture.as_str();
    cache_run(&places, &key(capture, "old-comparison"));
    let program = scratch.dir.join("program.sh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\necho '{{\"version\": \"{}\", \"capture_version\": \"{capture}\", \"commit\": \"c\", \"passed\": true, \"scenes\": [\"mac.a\"], \"boards\": {{\"mac.a\": \"h\"}}, \"findings\": []}}'\n",
            mac_versions().comparison
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let new = key(capture, &mac_versions().comparison);
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
    let capture = mac_versions().capture.as_str();
    cache_run(
        &places,
        &in_environment("25G241-27B52", capture, "old-comparison"),
    );
    let program = recheck_program(&scratch, capture, &mac_versions().comparison, true);
    let new = key(capture, &mac_versions().comparison);
    assert!(
        compare_again(&places, &new, &program).unwrap().is_none(),
        "the run of another environment is no source"
    );
    assert!(
        before_run(
            &places,
            "demo",
            COMMIT,
            Lane::MAC,
            ENVIRONMENT,
            "mac.a.",
            &program,
            &mut || Ok(Vec::new()),
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
        Lane::MAC,
        ENVIRONMENT,
        "mac.a.",
        &program,
        &mut || Ok(Vec::new()),
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
    let capture = mac_versions().capture.as_str();
    cache_run(&places, &key(capture, "old-comparison"));
    let recheck = recheck_program(&scratch, capture, &mac_versions().comparison, true);
    let concurrent_key = Key {
        prefix: "mac.a.".into(),
        ..key(capture, &mac_versions().comparison)
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
                Lane::MAC,
                ENVIRONMENT,
                "mac.a.",
                &program,
                &mut || Ok(Vec::new()),
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
            cache_run_at(&places, &concurrent_key, i64::MAX, 1);
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
    let capture = mac_versions().capture.as_str();
    cache_run(
        &places,
        &in_environment("25G241-27B52", capture, &mac_versions().comparison),
    );
    let program = recheck_program(&scratch, capture, &mac_versions().comparison, true);
    let found = lookup_newest(&places, "demo", COMMIT, Lane::MAC, "mac.", &program)
        .unwrap()
        .expect("a run of another environment is a commit's own run");
    assert_eq!(found.key.environment, "25G241-27B52");
    assert!(
        lookup_newest(&places, "demo", COMMIT, Lane::MAC, "mac.b.", &program)
            .unwrap()
            .is_none()
    );
}

/// Caches a base run of <prefix> that checked the scene <scene>.
fn cache_scene(places: &Places, prefix: &str, scene: &str, recorded_at: i64) {
    let key = Key {
        prefix: prefix.to_string(),
        ..key(&mac_versions().capture, &mac_versions().comparison)
    };
    let staging = shots_cache::stage(places, &key).unwrap();
    let report = staging.report();
    std::fs::create_dir_all(report.join("img")).unwrap();
    std::fs::write(report.join(format!("img/{scene}.shot.png")), scene).unwrap();
    std::fs::write(
        report.join("findings.json"),
        serde_json::json!({
            "version": key.versions.comparison,
            "capture_version": key.versions.capture,
            "commit": "c",
            "scenes": [scene],
            "boards": {scene: "h"},
            "seen": {scene: {"board": [], "shot": []}},
            "failed": [],
            "problems": 0,
            "sizes": {scene: {"board": {"width": 1.0, "height": 1.0}}},
            "components": [],
            "findings": [{"scene": scene, "level": "finding", "kind": "missing",
                "sentence": format!("{scene} differs")}],
        })
        .to_string(),
    )
    .unwrap();
    staging.finish_at(0, "text\n", recorded_at).unwrap();
}

/// <before_run> of <prefix> where the commit declares <declared>.
fn before_of(
    places: &Places,
    scratch: &Scratch,
    prefix: &str,
    declared: &[&str],
) -> Option<Cached> {
    before_run(
        places,
        "demo",
        COMMIT,
        Lane::MAC,
        ENVIRONMENT,
        prefix,
        &scratch.dir.join("no-program"),
        &mut || Ok(declared.iter().map(|name| name.to_string()).collect()),
    )
    .unwrap()
}

#[test]
fn saved_runs_that_together_check_every_declared_board_of_a_prefix_are_its_before_run() {
    let scratch = Scratch::new("compose-before");
    let places = scratch.places();
    let now = crate::app_timing::now();
    cache_scene(&places, "mac.a.one", "mac.a.one", now - 1);
    cache_scene(&places, "mac.a.two", "mac.a.two", now);
    let declared = ["mac.a.one", "mac.a.two", "mac.b.other"];
    let made = before_of(&places, &scratch, "mac.a.", &declared)
        .expect("the two saved runs hold every board the prefix selects");
    assert_eq!(made.key.prefix, "mac.a.");
    assert_eq!(made.exit_code(), 0);
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(made.findings_json()).unwrap()).unwrap();
    assert_eq!(
        file["scenes"],
        serde_json::json!(["mac.a.one", "mac.a.two"])
    );
    for scene in ["mac.a.one", "mac.a.two"] {
        for of_scene in ["boards", "seen", "sizes"] {
            assert!(!file[of_scene][scene].is_null(), "{of_scene}: {file}");
        }
        assert!(
            made.report()
                .join(format!("img/{scene}.shot.png"))
                .is_file()
        );
        assert!(
            made.findings.contains(&format!("{scene} (1)")),
            "{}",
            made.findings
        );
    }
    assert_eq!(file["findings"].as_array().unwrap().len(), 2);
    // The run is saved: the next build of the base finds it by itself.
    assert!(
        shots_cache::lookup(&places, &made.key).unwrap().is_some(),
        "the composed run is cached under the prefix"
    );
}

#[test]
fn a_saved_run_with_no_problem_count_makes_no_composed_run() {
    let scratch = Scratch::new("compose-no-count");
    let places = scratch.places();
    let now = crate::app_timing::now();
    cache_scene(&places, "mac.a.one", "mac.a.one", now);
    let part = Key {
        prefix: "mac.a.one".to_string(),
        ..key(&mac_versions().capture, &mac_versions().comparison)
    };
    let file = shots_cache::lookup(&places, &part)
        .unwrap()
        .expect("cached")
        .findings_json();
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("\"problems\":0,", "")).unwrap();
    let error = before_run(
        &places,
        "demo",
        COMMIT,
        Lane::MAC,
        ENVIRONMENT,
        "mac.a.",
        &scratch.dir.join("no-program"),
        &mut || Ok(vec!["mac.a.one".to_string()]),
    )
    .unwrap_err();
    let said = error.text();
    assert!(said.contains("problems"), "{said}");
}

#[test]
fn saved_runs_that_lack_a_declared_board_of_a_prefix_are_no_before_run_of_it() {
    let scratch = Scratch::new("compose-lacking");
    let places = scratch.places();
    cache_scene(&places, "mac.a.one", "mac.a.one", crate::app_timing::now());
    let declared = ["mac.a.one", "mac.a.two"];
    assert!(before_of(&places, &scratch, "mac.a.", &declared).is_none());
}

/// The versions that make the mac lane's runs.
fn mac_versions() -> &'static crate::shots_program::Versions {
    crate::shots_program::versions(boards::app_build::Platform::Mac)
}

/// A base build makes its run from saved runs long after it read them, so
/// the cap must keep them until then.
#[test]
fn the_saved_runs_a_base_build_will_make_its_run_from_stay_through_a_prune() {
    let scratch = Scratch::new("compose-kept");
    let places = scratch.places();
    cache_scene(&places, "mac.a.one", "mac.a.one", 1);
    let key = |prefix: &str| Key {
        prefix: prefix.to_string(),
        ..key(&mac_versions().capture, &mac_versions().comparison)
    };
    let part = shots_cache::run_folder(&places, &key("mac.a.one"));
    let saved = crate::shots_compose::Saved::of(&places, &key("mac.a."), &mut || {
        Ok(vec!["mac.a.one".to_string(), "mac.a.two".to_string()])
    })
    .unwrap()
    .expect("the run of mac.a.one gives a board of mac.a.");
    assert_eq!(saved.lacking().iter().collect::<Vec<_>>(), ["mac.a.two"]);
    assert!(crate::flock::is_held(&part).unwrap());
    drop(saved);
    assert!(!crate::flock::is_held(&part).unwrap());
}
