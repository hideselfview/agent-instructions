use super::*;
use crate::testing::Scratch;

fn file(path: &str, change: Change, lines: Option<(u64, u64)>) -> StagedFile {
    StagedFile {
        path: path.into(),
        change,
        lines,
    }
}

fn check(name: &str, outcome: Outcome, ms: Option<u64>) -> CheckRecord {
    CheckRecord {
        name: name.into(),
        kind: Kind::Check,
        outcome,
        handed: ms.map(|_| (1, 0)),
        ms,
    }
}

fn run(at: i64, total_ms: u64, passed: bool, checks: Vec<CheckRecord>) -> Run {
    Run {
        at,
        repo: "bae".into(),
        worktree: "/d/bae".into(),
        branch: Some("main".into()),
        base: None,
        passed,
        plan_ms: 10,
        total_ms,
        files: vec![
            file("src/a.rs", Change::Modified, Some((6, 1))),
            file("src/b.rs", Change::Added, Some((1, 0))),
        ],
        checks,
    }
}

fn database(scratch: &Scratch) -> std::path::PathBuf {
    scratch.dir.join(DATABASE)
}

#[test]
fn a_run_comes_back_whole_with_its_files_and_checks_in_order() {
    let scratch = Scratch::new("hook-db");
    let path = database(&scratch);
    assert_eq!(stats(&path, 0).unwrap(), None);
    let mut written = run(
        5,
        100,
        false,
        vec![
            check("fmt", Outcome::Passed, Some(40)),
            check("clippy", Outcome::Failed, Some(60)),
            check("docs", Outcome::Skipped, None),
        ],
    );
    written.branch = None;
    written.files.push(file("logo.png", Change::Deleted, None));
    record(&path, &written).unwrap();
    let read = stats(&path, 0).unwrap().unwrap();
    let mut expected = written;
    expected.files.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(read.recent, vec![expected]);
}

#[test]
fn recording_never_makes_the_folder() {
    let scratch = Scratch::new("hook-db-folder");
    let error = record(
        &scratch.dir.join("no-such-project").join(DATABASE),
        &run(1, 10, true, vec![]),
    )
    .unwrap_err()
    .text();
    assert!(error.contains("opening"), "{error}");
}

#[test]
fn a_database_with_another_schema_version_is_refused() {
    let scratch = Scratch::new("hook-db-version");
    let path = database(&scratch);
    record(&path, &run(1, 10, true, vec![])).unwrap();
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert!(
        record(&path, &run(2, 10, true, vec![]))
            .unwrap_err()
            .text()
            .contains("another builds schema")
    );
    assert!(
        stats(&path, 0)
            .unwrap_err()
            .text()
            .contains("another builds schema")
    );
}

#[test]
fn the_schema_refuses_what_the_closed_sets_and_ranges_forbid() {
    let scratch = Scratch::new("hook-db-checks");
    let path = database(&scratch);
    record(
        &path,
        &run(1, 10, true, vec![check("fmt", Outcome::Passed, Some(1))]),
    )
    .unwrap();
    let conn = open_for_writing(&path).unwrap();
    let refused = |sql: &str| conn.execute_batch(sql).is_err();
    // A run whose total is under its planning time, a passed value that is not 0 or 1.
    assert!(refused(
        "INSERT INTO hook_runs (started_at, repo, worktree, passed, plan_ms, total_ms) VALUES (1, 'r', 'w', 1, 9, 3)"
    ));
    assert!(refused(
        "INSERT INTO hook_runs (started_at, repo, worktree, passed, plan_ms, total_ms) VALUES (1, 'r', 'w', 2, 1, 3)"
    ));
    // A file or check of a run that does not exist.
    assert!(refused(
        "INSERT INTO hook_run_files (run_id, path, change, added, removed) VALUES (99, 'a', 'added', 1, 1)"
    ));
    // A change outside the closed set, a negative count, one count of a pair.
    assert!(refused(
        "INSERT INTO hook_run_files (run_id, path, change, added, removed) VALUES (1, 'a', 'renamed', 1, 1)"
    ));
    assert!(refused(
        "INSERT INTO hook_run_files (run_id, path, change, added, removed) VALUES (1, 'a', 'added', -1, 1)"
    ));
    assert!(refused(
        "INSERT INTO hook_run_files (run_id, path, change, added, removed) VALUES (1, 'a', 'added', 1, NULL)"
    ));
    // A kind or outcome outside the closed sets; a skipped check with a time; a ran check without one.
    assert!(refused(
        "INSERT INTO hook_run_checks (run_id, position, name, kind, outcome, files, crates, ms) VALUES (1, 5, 'x', 'test', 'passed', 1, 0, 1)"
    ));
    assert!(refused(
        "INSERT INTO hook_run_checks (run_id, position, name, kind, outcome, files, crates, ms) VALUES (1, 5, 'x', 'check', 'maybe', 1, 0, 1)"
    ));
    assert!(refused(
        "INSERT INTO hook_run_checks (run_id, position, name, kind, outcome, files, crates, ms) VALUES (1, 5, 'x', 'check', 'skipped', NULL, NULL, 4)"
    ));
    assert!(refused(
        "INSERT INTO hook_run_checks (run_id, position, name, kind, outcome, files, crates, ms) VALUES (1, 5, 'x', 'check', 'passed', 1, 0, NULL)"
    ));
    // A name twice in one run.
    assert!(refused(
        "INSERT INTO hook_run_checks (run_id, position, name, kind, outcome, files, crates, ms) VALUES (1, 5, 'fmt', 'check', 'passed', 1, 0, 1)"
    ));
    // Deleting a run takes its files and checks.
    conn.execute_batch("DELETE FROM hook_runs").unwrap();
    let left: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM hook_run_files) + (SELECT COUNT(*) FROM hook_run_checks)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn a_record_that_fails_midway_leaves_nothing() {
    let scratch = Scratch::new("hook-db-atomic");
    let path = database(&scratch);
    // Two checks of one name break the run's uniqueness after its row and files are written.
    let broken = run(
        1,
        10,
        true,
        vec![
            check("fmt", Outcome::Passed, Some(1)),
            check("fmt", Outcome::Passed, Some(2)),
        ],
    );
    assert!(record(&path, &broken).is_err());
    assert_eq!(stats(&path, 0).unwrap().unwrap().runs, 0);
}

#[test]
fn stats_give_each_check_its_median_slowest_failures_and_skips() {
    let scratch = Scratch::new("hook-db-stats");
    let path = database(&scratch);
    let runs = vec![
        run(
            1,
            1000,
            true,
            vec![
                check("fmt", Outcome::Passed, Some(100)),
                check("clippy", Outcome::Passed, Some(800)),
            ],
        ),
        run(
            2,
            3000,
            false,
            vec![
                check("fmt", Outcome::Passed, Some(300)),
                check("clippy", Outcome::Failed, Some(2500)),
            ],
        ),
        run(
            3,
            200,
            true,
            vec![
                check("fmt", Outcome::Passed, Some(200)),
                check("clippy", Outcome::Skipped, None),
            ],
        ),
        run(
            4,
            400,
            true,
            vec![
                check("fmt", Outcome::Passed, Some(400)),
                check("clippy", Outcome::Skipped, None),
            ],
        ),
    ];
    for r in &runs {
        record(&path, r).unwrap();
    }
    let stats = stats(&path, 0).unwrap().unwrap();
    assert_eq!(
        (stats.runs, stats.failed, stats.median_ms, stats.slowest_ms),
        (4, 1, 700, 3000)
    );
    let figures: Vec<_> = stats
        .checks
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.ran,
                c.failed,
                c.skipped,
                c.median_ms,
                c.slowest_ms,
            )
        })
        .collect();
    // Slowest median first: fmt's four times are 100, 200, 300, 400; clippy's two ran 800 and 2500.
    assert_eq!(
        figures,
        vec![("clippy", 2, 1, 2, 1650, 2500), ("fmt", 4, 0, 0, 250, 400)]
    );
    assert_eq!(
        stats.slowest.iter().map(|r| r.at).collect::<Vec<_>>(),
        vec![2, 1, 4, 3]
    );
    assert_eq!(
        stats.recent.iter().map(|r| r.at).collect::<Vec<_>>(),
        vec![4, 3, 2, 1]
    );
    let since = self::stats(&path, 3).unwrap().unwrap();
    assert_eq!((since.runs, since.median_ms), (2, 300));
    assert_eq!(
        since
            .checks
            .iter()
            .find(|c| c.name == "clippy")
            .map(|c| (c.ran, c.skipped, c.median_ms)),
        Some((0, 2, 0))
    );
}

#[test]
fn the_report_names_the_checks_and_the_slowest_runs() {
    let scratch = Scratch::new("hook-db-report");
    let path = database(&scratch);
    record(
        &path,
        &run(
            1_700_000_000,
            2500,
            true,
            vec![
                check("fmt", Outcome::Passed, Some(100)),
                check("clippy", Outcome::Skipped, None),
            ],
        ),
    )
    .unwrap();
    let text = report("bae", Some("2026-11-01"), stats(&path, 0).unwrap().as_ref());
    assert!(
        text.starts_with(
            "project bae, runs since 2026-11-01: 1 run, 0 failed, median 2.5s, slowest 2.5s\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("fmt") && text.contains("clippy") && text.contains("2 files +7 -1 (rs 2)"),
        "{text}"
    );
    assert_eq!(
        report("bae", None, None),
        "project bae, all runs: none recorded\n"
    );
}

#[test]
fn a_day_starts_at_local_midnight_and_a_bad_day_is_none() {
    let start = crate::text::local_start_of("2026-11-13").unwrap();
    assert_eq!(crate::text::local_minute(start), "2026-11-13 00:00");
    assert_eq!(crate::text::local_start_of("2026-02-30"), None);
    assert_eq!(crate::text::local_start_of("yesterday"), None);
}

#[test]
fn the_dashboard_json_has_each_project_with_its_figures_and_the_latest_runs_first() {
    let scratch = Scratch::new("hook-json");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["bae", "coven"]);
    let json: serde_json::Value = serde_json::from_str(&dashboard_json(&places)).unwrap();
    assert_eq!(json["projects"][0]["runs"], 0);
    for at in 1..=25 {
        record(
            &store.dir.join(DATABASE),
            &run(
                at,
                at as u64 * 10,
                true,
                vec![check("fmt", Outcome::Passed, Some(5))],
            ),
        )
        .unwrap();
    }
    let json: serde_json::Value = serde_json::from_str(&dashboard_json(&places)).unwrap();
    let project = &json["projects"][0];
    assert_eq!(
        (project["project"].as_str(), project["runs"].as_u64()),
        (Some("demo"), Some(25))
    );
    let recent = project["recent"].as_array().unwrap();
    assert_eq!(
        (
            recent.len(),
            recent[0]["at"].as_i64(),
            recent[19]["at"].as_i64()
        ),
        (RECENT_RUNS, Some(25), Some(6))
    );
    assert_eq!(recent[0]["kinds"], serde_json::json!({"rs": 2}));
    assert_eq!(project["checks"][0]["name"], "fmt");
    std::fs::write(
        store.dir.join(DATABASE),
        "not a database, and long enough to have a header of sorts",
    )
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&dashboard_json(&places)).unwrap();
    assert!(json["projects"][0]["error"].is_string(), "{json}");
}

fn check_set(names: &[(&str, u64)]) -> Vec<CheckRecord> {
    names
        .iter()
        .map(|(name, ms)| check(name, Outcome::Passed, Some(*ms)))
        .collect()
}

#[test]
fn a_run_records_the_commit_it_started_on() {
    let scratch = Scratch::new("hook-db-base");
    let path = database(&scratch);
    let mut written = run(5, 100, true, vec![]);
    written.base = Some("abc123".into());
    record(&path, &written).unwrap();
    assert_eq!(
        stats(&path, 0).unwrap().unwrap().recent[0].base.as_deref(),
        Some("abc123")
    );
}

#[test]
fn a_set_of_checks_is_flagged_while_its_newest_run_is_over_the_limit() {
    let scratch = Scratch::new("hook-db-limit");
    let path = database(&scratch);
    let flagged = |path: &Path| {
        stats(path, 0)
            .unwrap()
            .unwrap()
            .over_limit
            .iter()
            .map(|r| r.at)
            .collect::<Vec<_>>()
    };
    record(
        &path,
        &run(
            1,
            50_000,
            true,
            check_set(&[("clippy", 40_000), ("fmt", 1_000)]),
        ),
    )
    .unwrap();
    record(&path, &run(2, 46_000, true, check_set(&[("fmt", 1_000)]))).unwrap();
    // A run under the limit of another set of checks ends neither.
    record(&path, &run(3, 5_000, true, check_set(&[("docs", 100)]))).unwrap();
    assert_eq!(flagged(&path), vec![1, 2]);
    // A slower run of a set replaces its older flag: one flag per set.
    record(
        &path,
        &run(
            4,
            90_000,
            true,
            check_set(&[("clippy", 80_000), ("fmt", 1_000)]),
        ),
    )
    .unwrap();
    assert_eq!(flagged(&path), vec![4, 2]);
    // A newer run of the set under the limit ends it; exactly at the limit is under.
    record(
        &path,
        &run(
            5,
            45_000,
            true,
            check_set(&[("fmt", 1_000), ("clippy", 30_000)]),
        ),
    )
    .unwrap();
    assert_eq!(flagged(&path), vec![2]);
    record(&path, &run(6, 3_000, true, check_set(&[("fmt", 1_000)]))).unwrap();
    assert_eq!(flagged(&path), Vec::<i64>::new());
    // A run that checked nothing is never flagged.
    record(&path, &run(7, 60_000, true, vec![])).unwrap();
    assert_eq!(flagged(&path), Vec::<i64>::new());
}

#[test]
fn a_check_gets_a_trend_of_its_latest_runs_against_the_ones_before() {
    let scratch = Scratch::new("hook-db-trend");
    let path = database(&scratch);
    for at in 1..=12 {
        let ms = if at <= 6 { 10_000 } else { 30_000 };
        record(&path, &run(at, ms, true, check_set(&[("clippy", ms)]))).unwrap();
    }
    // 12 runs: the latest ten against the two before them is too few to compare.
    assert_eq!(stats(&path, 0).unwrap().unwrap().checks[0].trend, None);
    for at in 13..=16 {
        record(
            &path,
            &run(at, 30_000, true, check_set(&[("clippy", 30_000)])),
        )
        .unwrap();
    }
    // Latest ten: runs 7 to 16, all 30s; the six before: all 10s.
    let trend = stats(&path, 0).unwrap().unwrap().checks[0].trend.unwrap();
    assert_eq!((trend.recent_ms, trend.earlier_ms), (30_000, 10_000));
    let text = report("bae", None, stats(&path, 0).unwrap().as_ref());
    assert!(text.contains("30.0s vs 10.0s"), "{text}");
}

#[test]
fn the_dashboard_json_carries_the_flags_and_the_trend() {
    let scratch = Scratch::new("hook-json-flags");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    record(
        &store.dir.join(DATABASE),
        &run(1, 99_000, true, check_set(&[("clippy", 90_000)])),
    )
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&dashboard_json(&places)).unwrap();
    let project = &json["projects"][0];
    assert_eq!(project["limit_ms"], 45_000);
    assert_eq!(project["over_limit"][0]["total_ms"], 99_000);
    assert_eq!(project["over_limit"][0]["worktree"], "/d/bae");
    assert_eq!(project["over_limit"][0]["checks"][0]["crates"], 0);
    assert!(project["checks"][0]["recent_ms"].is_null());
}
