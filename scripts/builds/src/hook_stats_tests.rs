use super::*;
use crate::testing::Scratch;

fn file(path: &str, change: Change, lines: Option<(u64, u64)>) -> StagedFile {
    StagedFile {
        path: path.into(),
        change,
        lines,
    }
}

/// A check that ran, passed or not, in <ms>, handed one file.
fn ran(name: &str, passed: bool, ms: u64) -> CheckRecord {
    CheckRecord {
        name: name.into(),
        kind: Kind::Check,
        outcome: Outcome::Ran {
            result: if passed {
                CheckResult::Passed
            } else {
                CheckResult::Failed(Some(crate::step_failure::StepFailure::default()))
            },
            handed: (1, 0),
            ms: Some(ms),
            started: None,
            cargo: None,
        },
    }
}

fn skipped(name: &str) -> CheckRecord {
    CheckRecord {
        name: name.into(),
        kind: Kind::Check,
        outcome: Outcome::Skipped,
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

/// A failed check comes back with what it said, one that printed nothing
/// with an empty failure, and one that builds kept nothing of as not kept.
#[test]
fn a_failed_check_comes_back_with_what_it_said_or_as_not_kept() {
    let scratch = Scratch::new("hook-db-failure");
    let path = database(&scratch);
    let failed = |name: &str, result: CheckResult| {
        let mut check = ran(name, false, 60);
        if let Outcome::Ran { result: r, .. } = &mut check.outcome {
            *r = result;
        }
        check
    };
    let written = run(
        5,
        100,
        false,
        vec![
            failed(
                "clippy",
                CheckResult::Failed(Some(crate::step_failure::StepFailure {
                    excerpt: Some("error: unused variable".into()),
                    failed: vec!["src/a.rs".into(), "src/b.rs".into()],
                })),
            ),
            failed(
                "fmt",
                CheckResult::Failed(Some(crate::step_failure::StepFailure::default())),
            ),
            failed("docs", CheckResult::Failed(None)),
        ],
    );
    record(&path, &written).unwrap();
    let mut expected = written;
    expected.files.sort_by(|a, b| a.path.cmp(&b.path));
    // A failed check's time is not kept.
    for check in &mut expected.checks {
        if let Outcome::Ran { ms, .. } = &mut check.outcome {
            *ms = None;
        }
    }
    assert_eq!(recorded_runs(&path, "bae"), vec![expected]);
}

#[test]
fn a_run_comes_back_whole_with_its_files_and_checks_in_order() {
    let scratch = Scratch::new("hook-db");
    let path = database(&scratch);
    assert_eq!(stats(&path, 0, "bae").unwrap(), None);
    let mut written = run(
        5,
        100,
        true,
        vec![
            ran("fmt", true, 40),
            ran("clippy", true, 60),
            skipped("docs"),
        ],
    );
    written.branch = None;
    written.files.push(file("logo.png", Change::Deleted, None));
    record(&path, &written).unwrap();
    let read = stats(&path, 0, "bae").unwrap().unwrap();
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
        stats(&path, 0, "bae")
            .unwrap_err()
            .text()
            .contains("another builds schema")
    );
}

#[test]
fn the_schema_refuses_what_the_closed_sets_and_ranges_forbid() {
    let scratch = Scratch::new("hook-db-checks");
    let path = database(&scratch);
    record(&path, &run(1, 10, true, vec![ran("fmt", true, 1)])).unwrap();
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
    let broken = run(1, 10, true, vec![ran("fmt", true, 1), ran("fmt", true, 2)]);
    assert!(record(&path, &broken).is_err());
    assert_eq!(stats(&path, 0, "bae").unwrap().unwrap().runs, 0);
}

#[test]
fn stats_give_runs_their_figures_from_passing_runs_and_each_check_its_figures_from_its_passes() {
    let scratch = Scratch::new("hook-db-stats");
    let path = database(&scratch);
    let runs = vec![
        run(
            1,
            1000,
            true,
            vec![ran("fmt", true, 100), ran("clippy", true, 800)],
        ),
        run(
            2,
            3000,
            false,
            vec![ran("fmt", true, 300), ran("clippy", false, 2500)],
        ),
        run(3, 200, true, vec![ran("fmt", true, 200), skipped("clippy")]),
        run(4, 400, true, vec![ran("fmt", true, 400), skipped("clippy")]),
    ];
    for r in &runs {
        record(&path, r).unwrap();
    }
    let stats = stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(
        (stats.runs, stats.median_ms, stats.slowest_ms),
        (3, 400, 1000),
        "the failed run (3 s) is in no run figure"
    );
    let figures: Vec<_> = stats
        .checks
        .iter()
        .map(|c| (c.name.as_str(), c.ran, c.skipped, c.median_ms, c.slowest_ms))
        .collect();
    // Slowest median first: fmt passed in all four runs, at 100, 300, 200 and 400 (the failed
    // run's fmt pass counts); clippy passed once, at 800 (its failed 2500 is left out).
    assert_eq!(
        figures,
        vec![("clippy", 1, 2, 800, 800), ("fmt", 4, 0, 250, 400)]
    );
    assert_eq!(
        stats.slowest.iter().map(|r| r.at).collect::<Vec<_>>(),
        vec![1, 4, 3]
    );
    assert_eq!(
        stats.recent.iter().map(|r| r.at).collect::<Vec<_>>(),
        vec![4, 3, 1]
    );
    let since = self::stats(&path, 3, "bae").unwrap().unwrap();
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
            vec![ran("fmt", true, 100), skipped("clippy")],
        ),
    )
    .unwrap();
    let text = report(
        "bae",
        "bae",
        Some("2026-11-01"),
        stats(&path, 0, "bae").unwrap().as_ref(),
    );
    assert!(
        text.starts_with(
            "project bae, repo bae, runs since 2026-11-01: 1 passing run, median 2.5s, slowest 2.5s\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("fmt") && text.contains("clippy") && text.contains("2 files +7 -1 (rs 2)"),
        "{text}"
    );
    assert_eq!(
        report("bae", "bae", None, None),
        "project bae, repo bae, all runs: none recorded\n"
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
    let json: serde_json::Value = serde_json::from_str(&dashboard_json_at(&places, 100)).unwrap();
    assert_eq!(json["projects"][0]["repos"], serde_json::json!([]));
    for at in 1..=25 {
        record(
            &store.dir.join(DATABASE),
            &run(at, at as u64 * 10, true, vec![ran("fmt", true, 5)]),
        )
        .unwrap();
    }
    let json: serde_json::Value = serde_json::from_str(&dashboard_json_at(&places, 100)).unwrap();
    let project = &json["projects"][0]["repos"][0];
    assert_eq!(
        (
            json["projects"][0]["project"].as_str(),
            project["repo"].as_str(),
            project["runs"].as_u64()
        ),
        (Some("demo"), Some("bae"), Some(25))
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
    let json: serde_json::Value = serde_json::from_str(&dashboard_json_at(&places, 100)).unwrap();
    assert!(json["projects"][0]["error"].is_string(), "{json}");
}

fn check_set(names: &[(&str, u64)]) -> Vec<CheckRecord> {
    names
        .iter()
        .map(|(name, ms)| ran(name, true, *ms))
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
        stats(&path, 0, "bae").unwrap().unwrap().recent[0]
            .base
            .as_deref(),
        Some("abc123")
    );
}

#[test]
fn a_set_of_checks_is_flagged_while_its_newest_run_is_over_the_limit() {
    let scratch = Scratch::new("hook-db-limit");
    let path = database(&scratch);
    let flagged = |path: &Path| {
        stats(path, 0, "bae")
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
    assert_eq!(
        stats(&path, 0, "bae").unwrap().unwrap().checks[0].trend,
        None
    );
    for at in 13..=16 {
        record(
            &path,
            &run(at, 30_000, true, check_set(&[("clippy", 30_000)])),
        )
        .unwrap();
    }
    // Latest ten: runs 7 to 16, all 30s; the six before: all 10s.
    let trend = stats(&path, 0, "bae").unwrap().unwrap().checks[0]
        .trend
        .unwrap();
    assert_eq!((trend.recent_ms, trend.earlier_ms), (30_000, 10_000));
    let text = report("bae", "bae", None, stats(&path, 0, "bae").unwrap().as_ref());
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
    let json: serde_json::Value = serde_json::from_str(&dashboard_json_at(&places, 100)).unwrap();
    let project = &json["projects"][0]["repos"][0];
    assert_eq!(project["limit_ms"], 45_000);
    assert_eq!(project["over_count"], 1);
    assert_eq!(project["over"][0]["total_ms"], 99_000);
    assert_eq!(project["over"][0]["worktree"], "/d/bae");
    assert_eq!(project["over"][0]["checks"][0]["crates"], 0);
    assert!(project["checks"][0]["recent_ms"].is_null());
}

#[test]
fn each_repos_runs_are_kept_apart_in_the_figures_and_the_dashboard() {
    let scratch = Scratch::new("hook-repos");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let path = store.dir.join(DATABASE);
    for (at, repo, total) in [
        (1, "bae", 10_000),
        (2, "bae", 30_000),
        (3, "agent-instructions", 90_000),
    ] {
        let mut r = run(at, total, true, check_set(&[("clippy", total)]));
        r.repo = repo.into();
        record(&path, &r).unwrap();
    }
    assert_eq!(repos(&path).unwrap(), ["agent-instructions", "bae"]);
    let bae = stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(
        (bae.runs, bae.median_ms, bae.slowest_ms),
        (2, 20_000, 30_000)
    );
    assert_eq!(bae.recent.len(), 2);
    assert!(
        bae.over_limit.is_empty(),
        "the other repo's slow run is not bae's"
    );
    let other = stats(&path, 0, "agent-instructions").unwrap().unwrap();
    assert_eq!((other.runs, other.over_limit.len()), (1, 1));
    let json: serde_json::Value = serde_json::from_str(&dashboard_json_at(&places, 100)).unwrap();
    let panels = json["projects"][0]["repos"].as_array().unwrap();
    let names: Vec<&str> = panels.iter().map(|p| p["repo"].as_str().unwrap()).collect();
    assert_eq!(names, ["agent-instructions", "bae"]);
    assert_eq!(panels[1]["runs"], 2);
    assert_eq!(panels[0]["over"].as_array().unwrap().len(), 1);
    assert_eq!(
        (
            panels[0]["over_count"].as_u64(),
            panels[1]["over_count"].as_u64()
        ),
        (Some(1), Some(0))
    );
}

#[test]
fn a_failed_slow_run_moves_no_median_slowest_trend_or_over_limit_list() {
    let scratch = Scratch::new("hook-db-passing-only");
    let path = database(&scratch);
    // Thirteen passing runs of one check at 10 s, then ones at 4 s.
    for at in 1..=13 {
        let ms = if at <= 3 { 10_000 } else { 4_000 };
        record(&path, &run(at, ms, true, check_set(&[("clippy", ms)]))).unwrap();
    }
    let before = stats(&path, 0, "bae").unwrap().unwrap();
    // A slow failed run, over the limit, with a slow failed check.
    record(
        &path,
        &run(14, 200_000, false, vec![ran("clippy", false, 190_000)]),
    )
    .unwrap();
    let after = stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(after, before);
    assert_eq!(
        (after.runs, after.median_ms, after.slowest_ms),
        (13, 4_000, 10_000)
    );
    assert!(after.over_limit.is_empty() && after.over.is_empty());
    assert_eq!(after.over_count, 0);
    let clippy = &after.checks[0];
    assert_eq!(
        (clippy.ran, clippy.median_ms, clippy.slowest_ms),
        (13, 4_000, 10_000)
    );
    assert!(clippy.trend.is_some());
    assert_eq!(after.recent.iter().map(|r| r.at).max(), Some(13));
}

#[test]
fn only_passing_runs_go_over_the_limit() {
    let scratch = Scratch::new("hook-db-over-passing");
    let path = database(&scratch);
    record(
        &path,
        &run(1, 99_000, true, check_set(&[("clippy", 90_000)])),
    )
    .unwrap();
    record(
        &path,
        &run(2, 150_000, false, check_set(&[("clippy", 140_000)])),
    )
    .unwrap();
    let stats = stats(&path, 0, "bae").unwrap().unwrap();
    assert_eq!(stats.over_count, 1);
    assert_eq!(stats.over.iter().map(|r| r.at).collect::<Vec<_>>(), vec![1]);
    assert_eq!(
        stats.over_limit.iter().map(|r| r.at).collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn a_run_where_one_check_fails_adds_its_passing_checks_and_no_run_time() {
    let scratch = Scratch::new("hook-db-one-check-fails");
    let path = database(&scratch);
    record(
        &path,
        &run(
            1,
            20_000,
            true,
            check_set(&[("clippy", 18_000), ("detekt", 1_000)]),
        ),
    )
    .unwrap();
    // clippy passed at 80 s and detekt failed at 100 s: the run took 200 s and failed.
    record(
        &path,
        &run(
            2,
            200_000,
            false,
            vec![ran("clippy", true, 80_000), ran("detekt", false, 100_000)],
        ),
    )
    .unwrap();
    let stats = stats(&path, 0, "bae").unwrap().unwrap();
    // The run adds nothing to the run median, the slowest run, the bars or the over-limit list.
    assert_eq!(
        (stats.runs, stats.median_ms, stats.slowest_ms),
        (1, 20_000, 20_000)
    );
    assert_eq!(
        stats.recent.iter().map(|r| r.at).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(stats.over_count, 0);
    assert!(stats.over.is_empty() && stats.over_limit.is_empty());
    // clippy's pass counts in its figures; detekt's failure counts in none.
    let find = |name: &str| stats.checks.iter().find(|c| c.name == name).unwrap();
    let clippy = find("clippy");
    assert_eq!(
        (clippy.ran, clippy.median_ms, clippy.slowest_ms),
        (2, 49_000, 80_000)
    );
    let detekt = find("detekt");
    assert_eq!(
        (detekt.ran, detekt.median_ms, detekt.slowest_ms),
        (1, 1_000, 1_000)
    );
}

#[test]
fn a_failed_checks_time_is_not_stored_and_a_passed_checks_time_is() {
    let scratch = Scratch::new("hook-db-failed-time");
    let path = database(&scratch);
    record(
        &path,
        &run(
            1,
            200_000,
            false,
            vec![ran("clippy", true, 80_000), ran("detekt", false, 100_000)],
        ),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let times: Vec<(String, Option<i64>)> = conn
        .prepare("SELECT name, ms FROM hook_run_checks ORDER BY position")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        times,
        vec![
            ("clippy".to_string(), Some(80_000)),
            ("detekt".to_string(), None)
        ]
    );
}

#[test]
fn the_dashboard_counts_only_runs_of_the_last_12_hours() {
    let scratch = Scratch::new("hook-json-window");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let now = 10_000_000;
    let hours = |n: i64| now - n * 3600;
    // Yesterday's runs: one slow, one fast. Today's: one fast.
    for (at, total) in [(hours(30), 99_000), (hours(13), 5_000), (hours(2), 7_000)] {
        record(
            &store.dir.join(DATABASE),
            &run(at, total, true, check_set(&[("clippy", total)])),
        )
        .unwrap();
    }
    let json: serde_json::Value = serde_json::from_str(&dashboard_json_at(&places, now)).unwrap();
    let repo = &json["projects"][0]["repos"][0];
    assert_eq!(repo["runs"], 1);
    assert_eq!(repo["over_count"], 0);
    assert_eq!(repo["median_ms"], 7_000);
    assert_eq!(repo["slowest_ms"], 7_000);
    assert_eq!(repo["recent"].as_array().unwrap().len(), 1);
    assert_eq!(repo["checks"][0]["ran"], 1);
    // Twelve hours later, today's run is yesterday's too.
    let later: serde_json::Value =
        serde_json::from_str(&dashboard_json_at(&places, now + 11 * 3600)).unwrap();
    assert_eq!(later["projects"][0]["repos"][0]["runs"], 0);
}

/// A check comes back with this Mac's load when it started, or with the host
/// and load of the `builds cargo` run it started: the reader matches the
/// cargo run to the check by the hook run's worktree, its start time and the
/// check's position.
#[test]
fn a_check_comes_back_with_where_it_ran_and_how_loaded_that_host_was() {
    let scratch = Scratch::new("hook-db-places");
    let path = database(&scratch);
    let here = Load {
        cores: 14,
        one_minute: 21.2,
    };
    let started_here = |mut check: CheckRecord| {
        if let Outcome::Ran { started, .. } = &mut check.outcome {
            *started = Some(here);
        }
        check
    };
    let fmt = started_here(ran("fmt", true, 40));
    let clippy = started_here(ran("clippy", true, 122_800));
    let written = run(7, 123_000, true, vec![fmt, clippy]);
    record(&path, &written).unwrap();
    let cargo = |position: u32, lock_wait: Option<u64>| crate::cargo_run::CargoRun {
        worktree: "/d/bae".into(),
        started_at: 8,
        subcommand: Some("clippy".into()),
        host: "m1".into(),
        load: Load {
            cores: 10,
            one_minute: 3.4,
        },
        lock_wait: lock_wait.map(std::time::Duration::from_millis),
        took: std::time::Duration::from_secs(120),
        hook_check: Some((7, position)),
    };
    crate::cargo_run::record_run(
        &path,
        &cargo(1, Some(12_000)),
        &[],
        &crate::cargo_run_outcome::Outcome::passed(),
        None,
    )
    .unwrap();
    // A cargo run of another hook run names no check of this one.
    let mut other = cargo(0, None);
    other.hook_check = Some((6, 0));
    crate::cargo_run::record_run(
        &path,
        &other,
        &[],
        &crate::cargo_run_outcome::Outcome::passed(),
        None,
    )
    .unwrap();
    let read = recorded_runs(&path, "bae");
    let places: Vec<Option<String>> = read[0].checks.iter().map(CheckRecord::place_text).collect();
    assert_eq!(
        places,
        [
            Some("on local, load 21/14".to_string()),
            Some("on m1, load 3/10, 12.0s waiting for cargo's locks".to_string()),
        ]
    );
    assert!(
        handed_text(&read[0])
            .contains("clippy 122.8s (on m1, load 3/10, 12.0s waiting for cargo's locks)"),
        "{}",
        handed_text(&read[0])
    );
}
