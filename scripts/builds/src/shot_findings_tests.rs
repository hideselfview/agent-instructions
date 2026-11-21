use super::*;
use crate::shots_cache;

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
fn the_line_that_counts_the_pairs_the_base_never_rendered_is_told_from_the_others() {
    let line = "builds shots: 2 pair(s) the base never rendered, with 3 finding(s) that fail nothing: mac.a.b, mac.a.c";
    assert!(says_unrendered(line));
    assert_eq!(failing_shots(line), None);
    assert!(!has_findings(1, line));
    assert!(!says_unrendered("builds shots: no pair fails the run"));
}

#[test]
fn a_run_has_findings_when_it_passed_or_said_that_pairs_fail_it() {
    assert!(has_findings(0, ""));
    assert!(has_findings(1, "bae-shots: 2 pair(s) fail the run: a, b\n"));
    assert!(!has_findings(1, "bae-shots: the simulator did not start\n"));
}

#[test]
fn a_long_run_is_cached_whole() {
    let long = format!(
        "{}\nbae-shots: no pair fails the run\n",
        "é".repeat(1 << 21)
    );
    assert_eq!(findings_of(&long), long);
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
    let pair = r#"<img src="img/x.board.png"><img src="img/x.shot.png">"#;
    assert_eq!(
        report_files(pair),
        vec![
            "img/x.board.png",
            "img/x.board.json",
            "img/x.shot.png",
            "img/x.shot.json"
        ]
    );
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
    let into = scratch.dir.join("kept/first");
    copy_report(&from, &into).unwrap();
    assert!(into.join("img/a.png").is_file());
    assert!(!into.join(crate::shot_causes::FILE).exists());
    std::fs::write(from.join(crate::shot_causes::FILE), "{}").unwrap();
    let into = scratch.dir.join("kept/second");
    copy_report(&from, &into).unwrap();
    assert_eq!(
        std::fs::read_to_string(into.join(crate::shot_causes::FILE)).unwrap(),
        "{}"
    );
}

#[test]
fn a_run_grouped_by_cause_reads_the_records_its_cached_report_holds() {
    let scratch = crate::testing::Scratch::new("shots-causes");
    let places = scratch.places();
    let key = Key {
        repo: "demo".to_string(),
        commit: "a".repeat(40),
        lane: Lane::Mac,
        prefix: "mac.".to_string(),
        versions: crate::shots_program::Versions {
            capture: "c".repeat(16),
            comparison: "b".repeat(16),
        },
    };
    let cached = shots_cache::stage(&places, &key)
        .unwrap()
        .finish(1, "FAIL x\n")
        .unwrap();
    let missing = by_cause(&cached, "main").expect_err("refused").text();
    assert!(
        missing.contains("builds app-build demo main --all-shots mac --fresh"),
        "{missing}"
    );
    std::fs::create_dir_all(cached.report()).unwrap();
    std::fs::write(
        cached.findings_json(),
        r#"{"findings": [{"scene": "mac.a", "level": "warning", "component": "MacRow", "kind": "region", "sentence": "s"}], "scenes": ["mac.a"]}"#,
    )
    .unwrap();
    let text = by_cause(&cached, "main").unwrap();
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

#[test]
fn a_reports_findings_name_the_version_that_made_them() {
    let scratch = crate::testing::Scratch::new("shots-version");
    std::fs::write(
        scratch.dir.join(crate::shot_causes::FILE),
        r#"{"version": "v1", "capture_version": "c1", "commit": "c", "scenes": [], "findings": []}"#,
    )
    .unwrap();
    assert_eq!(
        versions_in(&scratch.dir).unwrap(),
        crate::shots_program::Versions {
            capture: "c1".to_string(),
            comparison: "v1".to_string(),
        }
    );
    std::fs::write(scratch.dir.join(crate::shot_causes::FILE), "{}").unwrap();
    assert!(
        versions_in(&scratch.dir)
            .unwrap_err()
            .text()
            .contains("names no shots checker versions")
    );
}
