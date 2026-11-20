use super::*;

#[test]
fn the_findings_are_the_text_after_cargos_running_line_or_all_of_it_without_one() {
    let output = "   Compiling a\n    Finished dev\n     Running `target/debug/bae-shots run`\nFAIL x: y\nbae-shots: done\n";
    assert_eq!(findings_of(output), "FAIL x: y\nbae-shots: done\n");
    assert_eq!(findings_of("FAIL x: y\n"), "FAIL x: y\n");
}

#[test]
fn the_failing_shots_come_from_the_line_that_names_the_pairs_that_fail() {
    assert_eq!(
        failing_shots("builds shots: 2 pair(s) fail the run: mac.a.b, mac.a.c.dark"),
        Some(vec!["mac.a.b".to_string(), "mac.a.c.dark".to_string()])
    );
    assert_eq!(failing_shots("builds shots: no pair fails the run"), None);
}

#[test]
fn a_run_has_findings_when_it_passed_or_said_that_pairs_fail_it() {
    assert!(has_findings(0, ""));
    assert!(has_findings(1, "bae-shots: 2 pair(s) fail the run: a, b\n"));
    assert!(!has_findings(1, "bae-shots: the simulator did not start\n"));
}

#[test]
fn a_long_run_is_kept_whole() {
    let long = format!(
        "{}\nbae-shots: no pair fails the run\n",
        "é".repeat(1 << 21)
    );
    assert_eq!(findings_of(&long), long);
}

#[test]
fn a_stored_run_is_found_by_its_whole_key_and_a_later_run_of_the_key_replaces_it() {
    let scratch = crate::testing::Scratch::new("shots-key");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let database = store.dir.join(app_timing::DATABASE);
    let key = Key {
        repo: "demo".to_string(),
        commit: "a".repeat(40),
        lane: Lane::Mac,
        prefix: "mac.a.".to_string(),
        version: "b".repeat(40),
    };
    assert!(lookup(&database, &key).unwrap().is_none());
    store_run(&database, &key, 1, "first\n");
    store_run(&database, &key, 0, "second\n");
    let found = lookup(&database, &key).unwrap().expect("stored");
    assert_eq!((found.exit_code, found.findings.as_str()), (0, "second\n"));
    for other in [
        Key {
            commit: "c".repeat(40),
            ..key.clone()
        },
        Key {
            lane: Lane::Android,
            ..key.clone()
        },
        Key {
            prefix: "mac.b.".to_string(),
            ..key.clone()
        },
        Key {
            version: "c".repeat(40),
            ..key.clone()
        },
        Key {
            repo: "other".to_string(),
            ..key.clone()
        },
    ] {
        assert!(lookup(&database, &other).unwrap().is_none(), "{other:?}");
    }
}

fn store_run(database: &Path, key: &Key, exit: i64, findings: &str) {
    store(database, key, exit, findings).unwrap();
}

#[test]
fn the_report_folder_and_its_pictures_are_read_from_the_shots_programs_lines_and_page() {
    for program in ["bae-shots", "builds shots"] {
        let output = format!("x\n{program}: report at /w/side-1/shots/report/index.html\n");
        assert_eq!(
            report_dir_in(&output).as_deref(),
            Some("/w/side-1/shots/report")
        );
    }
    assert_eq!(report_dir_in("no report here\n"), None);
    let html = r#"<img src="img/a.png"><img src="img/a.png"><img src="/etc/x"><img src="../up.png"><img src="https://x/y.png"><img src="img/b c.png">"#;
    assert_eq!(report_files(html), vec!["img/a.png", "img/b c.png"]);
}

#[test]
fn only_the_newest_reports_are_kept() {
    let scratch = crate::testing::Scratch::new("shots-prune");
    let shots = scratch.dir.join("shots");
    for n in 0..KEEP_REPORTS + 3 {
        let folder = shots.join(format!("run-{n:03}"));
        std::fs::create_dir_all(&folder).unwrap();
        crate::testing::age(&folder, (KEEP_REPORTS + 3 - n) as u64);
    }
    prune_reports(&shots);
    let left = std::fs::read_dir(&shots).unwrap().count();
    assert_eq!(left, KEEP_REPORTS);
    assert!(!shots.join("run-000").exists() && shots.join("run-032").exists());
}

#[test]
fn a_missing_run_of_a_lanes_whole_prefix_is_made_with_all_shots() {
    assert_eq!(
        how_to_make("bae", "main", Lane::Mac, "mac."),
        "builds app-build bae main --all-shots mac"
    );
    assert_eq!(
        how_to_make("bae", "main", Lane::Android, "android.welcome."),
        "builds app-build bae main --shots android.welcome."
    );
}

#[test]
fn a_report_is_copied_with_its_pictures_and_its_findings_records_when_it_has_them() {
    let scratch = crate::testing::Scratch::new("shots-copy");
    let from = scratch.dir.join("from");
    std::fs::create_dir_all(from.join("img")).unwrap();
    std::fs::write(from.join("index.html"), r#"<img src="img/a.png">"#).unwrap();
    std::fs::write(from.join("img/a.png"), "png").unwrap();
    let into = scratch.dir.join("kept/old");
    copy_report(&from, &into).unwrap();
    assert!(into.join("img/a.png").is_file());
    assert!(!into.join(crate::shot_causes::FILE).exists());
    std::fs::write(from.join(crate::shot_causes::FILE), "{}").unwrap();
    copy_report(&from, &into).unwrap();
    assert_eq!(
        std::fs::read_to_string(into.join(crate::shot_causes::FILE)).unwrap(),
        "{}"
    );
}

#[test]
fn a_run_grouped_by_cause_reads_the_records_its_kept_report_holds() {
    let scratch = crate::testing::Scratch::new("shots-causes");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let database = store.dir.join(app_timing::DATABASE);
    let key = Key {
        repo: "demo".to_string(),
        commit: "a".repeat(40),
        lane: Lane::Mac,
        prefix: "mac.".to_string(),
        version: "b".repeat(40),
    };
    let missing = by_cause(&database, &key, "main")
        .expect_err("refused")
        .text();
    assert!(
        missing.contains("builds app-build demo main --all-shots mac --fresh"),
        "{missing}"
    );
    let folder = report_folder(&database, &key);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join(crate::shot_causes::FILE),
        r#"{"findings": [{"scene": "mac.a", "component": "MacRow", "kind": "region", "sentence": "s"}], "scenes": ["mac.a"]}"#,
    )
    .unwrap();
    let text = by_cause(&database, &key, "main").unwrap();
    assert!(
        text.starts_with("1 finding in 1 of the 1 scene the run checked, grouped by cause"),
        "{text}"
    );
}

#[test]
fn the_script_that_copies_a_report_from_a_host_sends_its_page_pictures_and_records() {
    let scratch = crate::testing::Scratch::new("shots-copy-script");
    let from = scratch.dir.join("from dir");
    std::fs::create_dir_all(from.join("img")).unwrap();
    std::fs::write(from.join("index.html"), "page").unwrap();
    std::fs::write(from.join("img/a b.png"), "png").unwrap();
    for records in [false, true] {
        if records {
            std::fs::write(from.join(crate::shot_causes::FILE), "{}").unwrap();
        }
        let into = scratch.dir.join(format!("into-{records}"));
        std::fs::create_dir_all(&into).unwrap();
        let mut send = std::process::Command::new("sh");
        send.args([
            "-c",
            &report_copy_script(from.to_str().unwrap(), &["img/a b.png".to_string()]),
        ]);
        crate::remote::unpack(send, &into, "the test report").unwrap();
        assert!(into.join("index.html").is_file() && into.join("img/a b.png").is_file());
        assert_eq!(into.join(crate::shot_causes::FILE).is_file(), records);
    }
}

/// Stores a run of <key> in <database> with its report folder, which holds a
/// findings.json when <with_findings>.
fn store_with_report(database: &Path, key: &Key, with_findings: bool) {
    store_run(database, key, 1, "FAIL x: y\n");
    let folder = report_folder(database, key);
    std::fs::create_dir_all(&folder).unwrap();
    if with_findings {
        std::fs::write(
            folder.join(crate::shot_causes::FILE),
            format!(r#"{{"version": "{}"}}"#, key.version),
        )
        .unwrap();
    }
}

#[test]
fn the_before_run_is_the_newest_run_of_the_version_whose_prefix_starts_the_runs_and_keeps_its_findings()
 {
    let scratch = crate::testing::Scratch::new("shots-before");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let database = store.dir.join(app_timing::DATABASE);
    let base = "a".repeat(40);
    let key = |prefix: &str, version: &str| Key {
        repo: "demo".to_string(),
        commit: base.clone(),
        lane: Lane::Mac,
        prefix: prefix.to_string(),
        version: version.to_string(),
    };
    let find = |prefix: &str| {
        before_run(&database, "demo", &base, Lane::Mac, prefix, "v1")
            .unwrap()
            .map(|before| before.key.prefix)
    };
    assert_eq!(find("mac.welcome."), None);
    store_with_report(&database, &key("mac.welcome.", "v2"), true);
    store_with_report(&database, &key("mac.settings.", "v1"), true);
    store_with_report(&database, &key("mac.welcome.a.", "v1"), true);
    assert_eq!(
        find("mac.welcome."),
        None,
        "another version's run, another screen's and a narrower prefix's do not stand in"
    );
    store_with_report(&database, &key("mac.", "v1"), true);
    assert_eq!(find("mac.welcome."), Some("mac.".to_string()));
    let before = before_run(&database, "demo", &base, Lane::Mac, "mac.welcome.", "v1")
        .unwrap()
        .unwrap();
    assert_eq!(
        before.findings,
        report_folder(&database, &key("mac.", "v1")).join(crate::shot_causes::FILE)
    );
    // A run stored later wins; one whose report keeps no findings.json does
    // not count. The run of mac. ended ten seconds before the runs below.
    db::open(&database, db::Make::Never)
        .unwrap()
        .execute(
            "UPDATE shot_findings SET recorded_at = recorded_at - 10 WHERE prefix = 'mac.'",
            [],
        )
        .unwrap();
    store_with_report(&database, &key("mac.welcome.", "v1"), false);
    assert_eq!(find("mac.welcome."), Some("mac.".to_string()));
    store_with_report(&database, &key("mac.welcome.", "v1"), true);
    assert_eq!(find("mac.welcome."), Some("mac.welcome.".to_string()));
}

#[test]
fn a_reports_findings_name_the_version_that_made_them() {
    let scratch = crate::testing::Scratch::new("shots-version");
    std::fs::write(
        scratch.dir.join(crate::shot_causes::FILE),
        r#"{"version": "v1", "commit": "c", "scenes": [], "findings": []}"#,
    )
    .unwrap();
    assert_eq!(version_in(&scratch.dir).unwrap(), "v1");
    std::fs::write(scratch.dir.join(crate::shot_causes::FILE), "{}").unwrap();
    assert!(
        version_in(&scratch.dir)
            .unwrap_err()
            .text()
            .contains("names no builds-shots version")
    );
}
