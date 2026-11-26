use super::*;
use crate::testing::Scratch;

fn load(cores: u32, one_minute: f64) -> Option<Load> {
    Some(Load { cores, one_minute })
}

#[test]
fn a_load_falls_in_the_band_of_its_load_per_core() {
    assert_eq!(band_of(load(14, 13.9)), Band::Under1);
    assert_eq!(band_of(load(14, 14.0)), Band::From1To2);
    assert_eq!(band_of(load(10, 25.0)), Band::From2To4);
    assert_eq!(band_of(load(14, 56.0)), Band::From4);
    assert_eq!(band_of(None), Band::NotRecorded);
}

fn wait(kind: Kind, day: i64, ms: u64, load: Option<Load>) -> Wait {
    Wait {
        kind,
        started_at: day * 100,
        ms,
        load,
    }
}

#[test]
fn rows_count_each_kind_per_day_for_all_loads_then_per_band() {
    let waits = vec![
        wait(Kind::Hook, 1, 10_000, load(14, 7.0)),
        wait(Kind::Hook, 1, 30_000, load(14, 7.0)),
        wait(Kind::Hook, 1, 200_000, load(14, 30.0)),
        wait(Kind::Hook, 2, 5_000, None),
        wait(Kind::AppBuildStep, 1, 60_000, load(10, 5.0)),
    ];
    let day_of = |at: i64| format!("day {}", at / 100);
    let seen: Vec<(Kind, String, Option<Band>, u32, u64, u64)> = rows(waits, &day_of)
        .into_iter()
        .map(|r| (r.kind, r.day, r.band, r.count, r.median_ms, r.total_ms))
        .collect();
    let day = |n: u32| format!("day {n}");
    assert_eq!(
        seen,
        vec![
            (Kind::Hook, day(1), None, 3, 30_000, 240_000),
            (Kind::Hook, day(1), Some(Band::Under1), 2, 20_000, 40_000),
            (
                Kind::Hook,
                day(1),
                Some(Band::From2To4),
                1,
                200_000,
                200_000
            ),
            (Kind::Hook, day(2), None, 1, 5_000, 5_000),
            (Kind::Hook, day(2), Some(Band::NotRecorded), 1, 5_000, 5_000),
            (Kind::AppBuildStep, day(1), None, 1, 60_000, 60_000),
            (
                Kind::AppBuildStep,
                day(1),
                Some(Band::Under1),
                1,
                60_000,
                60_000
            ),
        ]
    );
}

/// A store holds a hook run, a cargo run the hook's check started, a test
/// run, a clippy run no hook started and an app build run with two steps.
#[test]
fn a_store_gives_each_kind_its_own_rows_and_leaves_out_other_cargo_runs() {
    use crate::cargo_run::{CargoRun, record_run};
    use crate::hook_stats::{CheckRecord, Outcome, Run};
    let scratch = Scratch::new("waits-read");
    let path = scratch.dir.join(crate::db::DATABASE);
    let here = load(14, 28.0);
    crate::hook_stats::record(
        &path,
        &Run {
            at: 1_000,
            repo: "bae".into(),
            worktree: "/d/bae".into(),
            branch: None,
            base: None,
            passed: false,
            plan_ms: 100,
            total_ms: 50_000,
            files: Vec::new(),
            checks: vec![CheckRecord {
                name: "clippy".into(),
                kind: crate::checks::Kind::Check,
                outcome: Outcome::Ran {
                    result: crate::step_failure::RunResult::Failed(Some(
                        crate::step_failure::StepFailure::default(),
                    )),
                    handed: (1, 1),
                    ms: None,
                    started: here,
                    cargo: None,
                },
            }],
        },
    )
    .unwrap();
    let cargo = |subcommand: &str, ms: u64, hook_check: Option<(i64, u32)>| CargoRun {
        worktree: "/d/bae".into(),
        started_at: 1_001,
        subcommand: Some(subcommand.into()),
        host: "m1".into(),
        load: Load {
            cores: 10,
            one_minute: 5.0,
        },
        lock_wait: None,
        took: Duration::from_millis(ms),
        hook_check,
    };
    record_run(
        &path,
        &cargo("clippy", 40_000, Some((1_000, 0))),
        &[],
        &crate::cargo_run_outcome::Outcome::passed(),
        None,
    )
    .unwrap();
    record_run(
        &path,
        &cargo("nextest run", 90_000, None),
        &[],
        &crate::cargo_run_outcome::Outcome::passed(),
        None,
    )
    .unwrap();
    record_run(
        &path,
        &cargo("test", 30_000, None),
        &[],
        &crate::cargo_run_outcome::Outcome::passed(),
        None,
    )
    .unwrap();
    record_run(
        &path,
        &cargo("clippy", 20_000, None),
        &[],
        &crate::cargo_run_outcome::Outcome::passed(),
        None,
    )
    .unwrap();
    let step = |name: &str, wall_ms: u64| crate::app_timing::StepRecord {
        at_merge_base: None,
        name: name.into(),
        kind: crate::app_timing::StepKind::Step,
        started_at: 1_100,
        wall_ms,
        cpu_ms: 1,
        result: crate::app_timing::StepResult::Passed,
        shots: None,
    };
    crate::app_timing::record(
        &path,
        &crate::app_timing::RunRecord {
            started_at: 1_050,
            repo: "bae".into(),
            lane: crate::lane::Lane::MAC,
            checkout: "side-1".into(),
            built_ref: "branch".into(),
            requested_commit: None,
            built_commit: None,
            who: None,
            wait_ms: 7_000,
            outcome: crate::run_end::RunEnd::Passed,
            load: here,
            steps: vec![step("macos", 300_000), step("macos-tests", 100_000)],
        },
    )
    .unwrap();
    let mut all = read(&path, 0).unwrap();
    all.sort_by_key(|w| (w.kind, w.ms));
    let seen: Vec<(Kind, u64, Band)> = all
        .iter()
        .map(|w| (w.kind, w.ms, band_of(w.load)))
        .collect();
    assert_eq!(
        seen,
        vec![
            (Kind::Hook, 50_000, Band::From2To4),
            (Kind::TestRun, 30_000, Band::Under1),
            (Kind::TestRun, 90_000, Band::Under1),
            (Kind::AppBuildQueue, 7_000, Band::From2To4),
            (Kind::AppBuildStep, 100_000, Band::From2To4),
            (Kind::AppBuildStep, 300_000, Band::From2To4),
        ]
    );
    assert!(
        read(&path, 1_051)
            .unwrap()
            .iter()
            .all(|w| w.kind == Kind::AppBuildStep)
    );
    assert!(
        read(&scratch.dir.join("none.sqlite"), 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_report_prints_a_line_per_row_with_counts_medians_and_totals() {
    let day_of = |_: i64| "2026-11-16".to_string();
    let rows = rows(
        vec![
            wait(Kind::TestRun, 1, 90_000, load(14, 7.0)),
            wait(Kind::TestRun, 1, 4_000_000, load(14, 7.0)),
        ],
        &day_of,
    );
    let text = report("bae", Some("2026-11-16"), &rows);
    assert!(text.starts_with("project bae, since 2026-11-16:"), "{text}");
    assert!(
        text.contains("test run  2026-11-16  all loads          2  34m 05s   1h 08m"),
        "{text}"
    );
    assert!(text.contains("under 1"), "{text}");
    assert_eq!(
        report("bae", None, &[]),
        "project bae, all days: builds recorded no wait\n"
    );
}

#[test]
fn the_dashboard_shows_the_waits_of_today_and_yesterday() {
    let scratch = Scratch::new("waits-dashboard");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["bae"]);
    let path = store.dir.join(crate::db::DATABASE);
    let day = |text: &str| crate::text::local_start_of(text).unwrap();
    for (at, total_ms) in [
        (day("2026-11-14") + 3600, 1_000),
        (day("2026-11-16") + 3600, 2_000),
        (day("2026-11-17") + 3600, 3_000),
    ] {
        crate::hook_stats::record(
            &path,
            &crate::hook_stats::Run {
                at,
                repo: "bae".into(),
                worktree: "/d/bae".into(),
                branch: None,
                base: None,
                passed: true,
                plan_ms: 1,
                total_ms,
                files: Vec::new(),
                checks: Vec::new(),
            },
        )
        .unwrap();
    }
    let now = day("2026-11-17") + 12 * 3600;
    let json: serde_json::Value =
        serde_json::from_str(&crate::hook_stats::dashboard_json_at(&places, now)).unwrap();
    let rows = json["projects"][0]["waits"]["rows"].as_array().unwrap();
    let seen: Vec<(&str, &str, u64)> = rows
        .iter()
        .map(|row| {
            (
                row["day"].as_str().unwrap(),
                row["band"].as_str().unwrap(),
                row["total_ms"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            ("2026-11-16", "all loads", 2_000),
            ("2026-11-16", "not recorded", 2_000),
            ("2026-11-17", "all loads", 3_000),
            ("2026-11-17", "not recorded", 3_000),
        ]
    );
}

#[test]
fn yesterday_starts_at_its_own_local_midnight() {
    let day = |text: &str| crate::text::local_start_of(text).unwrap();
    assert_eq!(
        start_of_yesterday(day("2026-11-17") + 3600).unwrap(),
        day("2026-11-16")
    );
    assert_eq!(
        start_of_yesterday(day("2026-11-17")).unwrap(),
        day("2026-11-16")
    );
}
