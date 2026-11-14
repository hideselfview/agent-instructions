use super::*;
use crate::testing::Scratch;

/// A store file with the whole schema, in a scratch folder.
fn store(scratch: &Scratch) -> std::path::PathBuf {
    let path = scratch.dir.join(DATABASE);
    crate::db::open(&path, crate::db::Make::Allowed).unwrap();
    path
}

fn step(name: &str, kind: StepKind, at: i64, wall_ms: u64, result: StepResult) -> StepRecord {
    StepRecord {
        name: name.to_string(),
        kind,
        started_at: at,
        wall_ms,
        cpu_ms: wall_ms / 2,
        result,
    }
}

fn run(at: i64, lane: Lane, wait_ms: u64, passed: bool, steps: Vec<StepRecord>) -> RunRecord {
    RunRecord {
        started_at: at,
        repo: "bae".to_string(),
        lane,
        checkout: "side-1".to_string(),
        built_ref: "branch".to_string(),
        who: Some("agent".to_string()),
        wait_ms,
        passed,
        steps,
    }
}

const NOW: i64 = 1_000_000;

#[test]
fn a_recorded_run_comes_back_in_the_figures_with_its_steps() {
    let scratch = Scratch::new("timing-record");
    let path = store(&scratch);
    record(
        &path,
        &run(
            NOW - 100,
            Lane::Mac,
            2_000,
            true,
            vec![
                step(
                    "macos",
                    StepKind::Step,
                    NOW - 90,
                    60_000,
                    StepResult::Passed,
                ),
                step(
                    "test Foo",
                    StepKind::Test,
                    NOW - 30,
                    8_000,
                    StepResult::Passed,
                ),
                step(
                    "shots mac.a.",
                    StepKind::Shots,
                    NOW - 20,
                    4_000,
                    StepResult::Passed,
                ),
            ],
        ),
    )
    .unwrap();
    let repos = stats(&path, NOW - WINDOW_SECONDS).unwrap();
    assert_eq!(repos.len(), 1);
    let names: Vec<(&str, &str, u64)> = repos[0]
        .steps
        .iter()
        .map(|s| (s.name.as_str(), s.kind.as_str(), s.median_ms))
        .collect();
    assert_eq!(
        names,
        vec![
            ("macos", "step", 60_000),
            ("test Foo", "test", 8_000),
            ("shots mac.a.", "shots", 4_000)
        ]
    );
    assert_eq!(repos[0].steps[0].cpu_median_ms, 30_000);
    assert_eq!(repos[0].lanes.len(), 1);
    assert_eq!(repos[0].lanes[0].median_ms, 2_000);
    let wait = &repos[0].longest_waits[0];
    assert_eq!(
        (
            wait.who.as_deref(),
            wait.built_ref.as_str(),
            wait.checkout.as_str()
        ),
        (Some("agent"), "branch", "side-1")
    );
}

#[test]
fn step_figures_count_passing_runs_in_the_window_and_the_last_ten_are_the_newest() {
    let scratch = Scratch::new("timing-figures");
    let path = store(&scratch);
    // An old run outside the window, 12 passing runs of 1s..12s, and a failed run.
    record(
        &path,
        &run(
            NOW - WINDOW_SECONDS - 10,
            Lane::Mac,
            0,
            true,
            vec![step(
                "macos",
                StepKind::Step,
                NOW - WINDOW_SECONDS - 5,
                999_000,
                StepResult::Passed,
            )],
        ),
    )
    .unwrap();
    for n in 1..=12_u64 {
        record(
            &path,
            &run(
                NOW - 1000 + n as i64,
                Lane::Mac,
                0,
                true,
                vec![step(
                    "macos",
                    StepKind::Step,
                    NOW - 1000 + n as i64,
                    n * 1000,
                    StepResult::Passed,
                )],
            ),
        )
        .unwrap();
    }
    record(
        &path,
        &run(
            NOW - 5,
            Lane::Mac,
            0,
            false,
            vec![step(
                "macos",
                StepKind::Step,
                NOW - 5,
                500_000,
                StepResult::Failed,
            )],
        ),
    )
    .unwrap();
    let repo = &stats(&path, NOW - WINDOW_SECONDS).unwrap()[0];
    let macos = &repo.steps[0];
    assert_eq!(macos.ran, 12, "the old and the failed run are not counted");
    assert_eq!(macos.median_ms, 6_500);
    assert_eq!(macos.slowest_ms, 12_000);
    // The newest ten are 3s..12s.
    assert_eq!(macos.last_median_ms, 7_500);
    assert_eq!(
        repo.runs, 13,
        "the waits count every run in the window, the failed one too"
    );
}

#[test]
fn queue_waits_are_per_lane_and_the_longest_come_first() {
    let scratch = Scratch::new("timing-waits");
    let path = store(&scratch);
    for (n, wait) in [(1, 1_000), (2, 9_000), (3, 5_000)] {
        record(
            &path,
            &run(NOW - 100 + n, Lane::Mac, wait, true, Vec::new()),
        )
        .unwrap();
    }
    record(
        &path,
        &run(NOW - 50, Lane::Android, 40_000, false, Vec::new()),
    )
    .unwrap();
    let repo = &stats(&path, NOW - WINDOW_SECONDS).unwrap()[0];
    let lanes: Vec<(&str, u32, u64, u64)> = repo
        .lanes
        .iter()
        .map(|l| (l.lane.as_str(), l.runs, l.median_ms, l.slowest_ms))
        .collect();
    assert_eq!(
        lanes,
        vec![("mac", 3, 5_000, 9_000), ("android", 1, 40_000, 40_000)]
    );
    let longest: Vec<u64> = repo.longest_waits.iter().map(|w| w.wait_ms).collect();
    assert_eq!(longest, vec![40_000, 9_000, 5_000, 1_000]);
}

#[test]
fn a_store_with_no_run_in_the_window_has_no_figures_and_a_missing_file_has_none() {
    let scratch = Scratch::new("timing-empty");
    let path = store(&scratch);
    record(
        &path,
        &run(NOW - WINDOW_SECONDS - 1, Lane::Mac, 0, true, Vec::new()),
    )
    .unwrap();
    assert!(stats(&path, NOW - WINDOW_SECONDS).unwrap().is_empty());
    assert!(
        stats(&scratch.dir.join("none.sqlite"), 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_dashboard_json_names_the_window_and_each_repos_figures() {
    let scratch = Scratch::new("timing-json");
    let path = store(&scratch);
    record(
        &path,
        &run(
            NOW - 10,
            Lane::Android,
            3_000,
            true,
            vec![step(
                "android",
                StepKind::Step,
                NOW - 9,
                7_000,
                StepResult::Passed,
            )],
        ),
    )
    .unwrap();
    let json = dashboard_json(&path, NOW);
    assert_eq!(json["window_seconds"], WINDOW_SECONDS);
    let repo = &json["repos"][0];
    assert_eq!(repo["repo"], "bae");
    assert_eq!(repo["steps"][0]["name"], "android");
    assert_eq!(repo["steps"][0]["median_ms"], 7_000);
    assert_eq!(repo["lanes"][0]["slowest_ms"], 3_000);
    assert_eq!(repo["longest_waits"][0]["ref"], "branch");
}

#[test]
fn a_repo_in_a_project_has_that_projects_database_and_a_repo_in_none_has_none() {
    let scratch = Scratch::new("timing-database");
    let places = scratch.places();
    assert_eq!(database_of(&places, "bae").unwrap(), None);
    let store = crate::lists::fixtures::make_store(&places, &["bae", "coven"]);
    assert_eq!(
        database_of(&places, "bae").unwrap(),
        Some(store.dir.join(DATABASE))
    );
    assert_eq!(database_of(&places, "web").unwrap(), None);
}
