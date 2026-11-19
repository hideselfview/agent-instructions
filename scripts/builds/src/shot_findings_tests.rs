use super::*;

#[test]
fn the_findings_are_the_text_after_cargos_running_line_or_all_of_it_without_one() {
    let output = "   Compiling a\n    Finished dev\n     Running `target/debug/bae-shots run`\nFAIL x: y\nbae-shots: done\n";
    assert_eq!(findings_of(output), "FAIL x: y\nbae-shots: done\n");
    assert_eq!(findings_of("FAIL x: y\n"), "FAIL x: y\n");
}

#[test]
fn a_run_has_findings_when_it_passed_or_said_that_pairs_fail_it() {
    assert!(has_findings(0, ""));
    assert!(has_findings(1, "bae-shots: 2 pair(s) fail the run: a, b\n"));
    assert!(!has_findings(1, "bae-shots: the simulator did not start\n"));
}

#[test]
fn only_the_last_megabyte_of_a_long_run_is_kept_cut_at_a_character() {
    let long = "é".repeat(KEEP);
    let kept = findings_of(&long);
    assert!(kept.len() <= KEEP && kept.len() > KEEP - 4);
    assert!(kept.chars().all(|c| c == 'é'));
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
    let output = "x\nbae-shots: report at /w/side-1/shots/report/index.html\n";
    assert_eq!(
        report_dir_in(output).as_deref(),
        Some("/w/side-1/shots/report")
    );
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
