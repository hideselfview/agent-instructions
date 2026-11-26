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
        at_merge_base: None,
        name: name.to_string(),
        kind,
        started_at: at,
        wall_ms,
        cpu_ms: wall_ms / 2,
        result,
        shots: None,
    }
}

fn run(at: i64, lane: Lane, wait_ms: u64, outcome: RunEnd, steps: Vec<StepRecord>) -> RunRecord {
    RunRecord {
        started_at: at,
        repo: "bae".to_string(),
        lane,
        checkout: "side-1".to_string(),
        built_ref: "branch".to_string(),
        requested_commit: Some("f".repeat(40)),
        built_commit: None,
        who: Some("agent".to_string()),
        wait_ms,
        outcome,
        load: None,
        steps,
    }
}

fn record_checkout(run: &mut RunRecord, commit: String) {
    run.requested_commit = Some(commit.clone());
    run.built_commit = Some(commit.into());
}

const NOW: i64 = 1_000_000;

#[test]
fn cached_comparisons_do_not_change_when_a_build_without_steps_ended() {
    let scratch = Scratch::new("timing-reused-comparison");
    let path = store(&scratch);
    let mut built = run(NOW, Lane::MAC, 3_000, RunEnd::Failed, Vec::new());
    built.built_commit = Some(BuiltCommit {
        sha: "0123456789abcdef0123456789abcdef01234567".into(),
        reused_shots: vec![shots::Prefix::parse("mac.a.").unwrap()],
    });
    record(&path, &built).unwrap();
    assert_eq!(crate::app_timing::read_named_run(&path).unwrap(), built);
    assert_eq!(
        last_ends(&path, Lane::MAC).unwrap(),
        [("side-1".into(), NOW + 3)]
    );
}

#[test]
fn a_run_keeps_the_load_of_its_checkouts_host() {
    let scratch = Scratch::new("timing-load");
    let path = store(&scratch);
    let mut loaded = run(NOW, Lane::MAC, 0, RunEnd::Passed, Vec::new());
    loaded.load = Some(crate::host::Load {
        cores: 10,
        one_minute: 25.5,
    });
    record(&path, &loaded).unwrap();
    record(&path, &run(NOW, Lane::MAC, 0, RunEnd::Passed, Vec::new())).unwrap();
    let conn = crate::db::open_reader(&path).unwrap().unwrap();
    let loads: Vec<(Option<i64>, Option<f64>)> = conn
        .prepare("SELECT cores, load FROM app_build_runs ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(loads, vec![(Some(10), Some(25.5)), (None, None)]);
}

#[test]
fn a_run_keeps_the_commit_it_built_and_a_run_with_none_has_no_row() {
    let scratch = Scratch::new("timing-commit");
    let path = store(&scratch);
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let mut built = run(NOW, Lane::MAC, 0, RunEnd::Passed, Vec::new());
    record_checkout(&mut built, commit.to_string());
    record(&path, &built).unwrap();
    record(&path, &run(NOW, Lane::MAC, 0, RunEnd::Passed, Vec::new())).unwrap();
    let conn = crate::db::open_reader(&path).unwrap().unwrap();
    let commits: Vec<(i64, Option<String>)> = conn
        .prepare("SELECT r.id, c.built_commit FROM app_build_runs r LEFT JOIN app_build_run_commit c ON c.run_id = r.id ORDER BY r.id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(commits, vec![(1, Some(commit.to_string())), (2, None)]);
}

/// The passing runs of a repo's lane that recorded their commit come back
/// newest first, each with the steps that passed in it; failed and stopped
/// runs, runs of the other lane and runs with no commit do not.
#[test]
fn passing_runs_of_a_lane_come_back_newest_first_with_their_passed_steps() {
    let scratch = Scratch::new("timing-passed");
    let path = store(&scratch);
    let built = |commit: &str, lane: Lane, outcome: RunEnd, steps: Vec<StepRecord>| {
        let mut run = run(NOW, lane, 0, outcome, steps);
        record_checkout(&mut run, commit.repeat(40));
        run
    };
    let passing = |name: &str| step(name, StepKind::Step, NOW, 1, StepResult::Passed);
    record(
        &path,
        &built("a", Lane::MAC, RunEnd::Passed, vec![passing("macos")]),
    )
    .unwrap();
    record(
        &path,
        &built("b", Lane::MAC, RunEnd::Failed, vec![passing("macos")]),
    )
    .unwrap();
    record(
        &path,
        &built(
            "e",
            Lane::MAC,
            RunEnd::Stopped,
            vec![
                passing("macos"),
                step("macos-shots", StepKind::Step, NOW, 1, StepResult::Stopped),
            ],
        ),
    )
    .unwrap();
    record(
        &path,
        &built("c", Lane::ANDROID, RunEnd::Passed, vec![passing("android")]),
    )
    .unwrap();
    record(
        &path,
        &run(NOW, Lane::MAC, 0, RunEnd::Passed, vec![passing("macos")]),
    )
    .unwrap();
    record(
        &path,
        &built(
            "d",
            Lane::MAC,
            RunEnd::Passed,
            vec![
                passing("macos"),
                step("macos-shots", StepKind::Step, NOW, 1, StepResult::TakenOver),
                passing("macos-shots"),
            ],
        ),
    )
    .unwrap();
    let conn = crate::db::open_reader(&path).unwrap().unwrap();
    let runs = passed_runs(&conn, "bae", Lane::MAC, &path, Some("branch"), &[]).unwrap();
    assert_eq!(
        runs,
        vec![
            PassedRun {
                started_at: NOW,
                commit: "d".repeat(40).into(),
                steps: vec!["macos".to_string(), "macos-shots".to_string()],
            },
            PassedRun {
                started_at: NOW,
                commit: "a".repeat(40).into(),
                steps: vec!["macos".to_string()],
            },
        ]
    );
}

/// A passing build of the branch is found by the ref the caller named or by
/// the commit it built, among any number of newer passing runs of other
/// branches.
#[test]
fn a_branchs_passing_build_is_found_by_its_name_or_its_commit_whatever_ran_since() {
    let scratch = Scratch::new("timing-passed-branch");
    let path = store(&scratch);
    let passing = |name: &str| step(name, StepKind::Step, NOW, 1, StepResult::Passed);
    let build = |at: i64, named: &str, commit: &str| {
        let mut run = run(at, Lane::MAC, 0, RunEnd::Passed, vec![passing("macos")]);
        run.built_ref = named.to_string();
        record_checkout(&mut run, commit.repeat(40));
        run
    };
    record(&path, &build(NOW, "branch", "a")).unwrap();
    record(&path, &build(NOW + 1, "renamed", "b")).unwrap();
    for at in 0..60 {
        record(&path, &build(NOW + 2 + at, "someone-else", "c")).unwrap();
    }
    record(&path, &build(NOW + 100, "someone-else", "d")).unwrap();
    let conn = crate::db::open_reader(&path).unwrap().unwrap();
    let commits = |runs: Vec<PassedRun>| -> Vec<String> {
        runs.into_iter()
            .map(|run| run.commit.sha[..1].to_string())
            .collect()
    };
    let found = passed_runs(&conn, "bae", Lane::MAC, &path, Some("branch"), &[]).unwrap();
    assert_eq!(commits(found), ["a"]);
    let own = ["b".repeat(40)];
    let found = passed_runs(&conn, "bae", Lane::MAC, &path, Some("branch"), &own).unwrap();
    assert_eq!(commits(found), ["b", "a"]);
}

/// Whether a commit has a passing build that ran a shots prefix does not
/// depend on how many runs came after it.
#[test]
fn a_commits_passing_shots_run_is_found_whatever_ran_since() {
    let scratch = Scratch::new("timing-passed-shots");
    let path = store(&scratch);
    let build = |at: i64, commit: &str, steps: Vec<StepRecord>| {
        let mut run = run(at, Lane::MAC, 0, RunEnd::Passed, steps);
        record_checkout(&mut run, commit.repeat(40));
        run
    };
    let shots = step("shots mac.a.", StepKind::Shots, NOW, 1, StepResult::Passed);
    record(&path, &build(NOW, "a", vec![shots])).unwrap();
    for at in 0..60 {
        record(&path, &build(NOW + 1 + at, "b", Vec::new())).unwrap();
    }
    let records = BuildRecords {
        repo: "bae",
        stores: vec![RecordStore::open(path).unwrap()],
    };
    let ran = |commit: &str, prefix: &str| {
        records
            .has_passing_run(Lane::MAC, &commit.repeat(40), |run| run.ran_shots(prefix))
            .unwrap()
    };
    assert!(ran("a", "mac.a.") && !ran("a", "mac.b.") && !ran("b", "mac.a."));
}

/// bae's runs in the one database at <path>.
fn of_bae(path: &Path) -> [RepoDatabases; 1] {
    [RepoDatabases {
        repo: "bae".to_string(),
        paths: vec![path.to_path_buf()],
    }]
}

#[test]
fn a_recorded_run_comes_back_in_the_figures_with_its_steps() {
    let scratch = Scratch::new("timing-record");
    let path = store(&scratch);
    record(
        &path,
        &run(
            NOW - 100,
            Lane::MAC,
            2_000,
            RunEnd::Passed,
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
    let repos = stats(&of_bae(&path), NOW - WINDOW_SECONDS).unwrap();
    assert_eq!(repos.len(), 1);
    let mac = &repos[0].lanes[0];
    assert_eq!(
        (mac.lane.as_str(), mac.runs, mac.median_wait_ms),
        ("mac", 1, 2_000)
    );
    let names: Vec<(&str, StepKind, u64)> = mac
        .steps
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.median_ms))
        .collect();
    assert_eq!(
        names,
        vec![
            ("macos", StepKind::Step, 60_000),
            ("Foo", StepKind::Test, 8_000),
            ("mac.a.*", StepKind::Shots, 4_000)
        ]
    );
}

#[test]
fn a_shot_run_of_one_board_keeps_its_name_and_a_prefix_gets_a_star() {
    assert_eq!(
        step_word("shots mac.welcome.", StepKind::Shots),
        "mac.welcome.*"
    );
    assert_eq!(step_word("shots mac.", StepKind::Shots), "mac.*");
    assert_eq!(
        step_word("shots mac.component.progress", StepKind::Shots),
        "mac.component.progress"
    );
    assert_eq!(step_word("test FooTests", StepKind::Test), "FooTests");
    assert_eq!(step_word("macos-tests", StepKind::Step), "macos-tests");
}

#[test]
fn a_steps_figures_count_passing_runs_in_the_window_and_total_runs_times_median() {
    let scratch = Scratch::new("timing-figures");
    let path = store(&scratch);
    // An old run outside the window, 12 passing runs of 1s..12s, and a failed run.
    record(
        &path,
        &run(
            NOW - WINDOW_SECONDS - 10,
            Lane::MAC,
            0,
            RunEnd::Passed,
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
                Lane::MAC,
                0,
                RunEnd::Passed,
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
            Lane::MAC,
            0,
            RunEnd::Failed,
            vec![step(
                "macos",
                StepKind::Step,
                NOW - 5,
                500_000,
                StepResult::Failed(crate::step_failure::StepFailure::default()),
            )],
        ),
    )
    .unwrap();
    let lane = &stats(&of_bae(&path), NOW - WINDOW_SECONDS).unwrap()[0].lanes[0];
    let macos = &lane.steps[0];
    assert_eq!(macos.ran, 12, "the old and the failed run are not counted");
    assert_eq!(macos.median_ms, 6_500);
    assert_eq!(macos.slowest_ms, 12_000);
    assert_eq!(macos.total_ms, 78_000);
    assert_eq!(
        lane.runs, 13,
        "the waits count every run in the window, the failed one too"
    );
}

#[test]
fn steps_are_ranked_by_the_time_they_took_in_all() {
    let scratch = Scratch::new("timing-rank");
    let path = store(&scratch);
    // 5 runs of 2s (10s in all) outrank 1 run of 9s.
    for n in 0..5 {
        record(
            &path,
            &run(
                NOW - 100 + n,
                Lane::MAC,
                0,
                RunEnd::Passed,
                vec![step(
                    "fast",
                    StepKind::Step,
                    NOW - 100 + n,
                    2_000,
                    StepResult::Passed,
                )],
            ),
        )
        .unwrap();
    }
    record(
        &path,
        &run(
            NOW - 50,
            Lane::MAC,
            0,
            RunEnd::Passed,
            vec![step(
                "slow",
                StepKind::Step,
                NOW - 50,
                9_000,
                StepResult::Passed,
            )],
        ),
    )
    .unwrap();
    let lane = &stats(&of_bae(&path), NOW - WINDOW_SECONDS).unwrap()[0].lanes[0];
    let order: Vec<&str> = lane.steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(order, vec!["fast", "slow"]);
}

#[test]
fn queue_waits_are_per_lane_oldest_first_with_the_lanes_median() {
    let scratch = Scratch::new("timing-waits");
    let path = store(&scratch);
    for (n, wait) in [(1, 1_000), (2, 9_000), (3, 5_000)] {
        record(
            &path,
            &run(NOW - 100 + n, Lane::MAC, wait, RunEnd::Passed, Vec::new()),
        )
        .unwrap();
    }
    record(
        &path,
        &run(NOW - 50, Lane::ANDROID, 40_000, RunEnd::Failed, Vec::new()),
    )
    .unwrap();
    let repo = &stats(&of_bae(&path), NOW - WINDOW_SECONDS).unwrap()[0];
    let lanes: Vec<(String, u32, u64, Vec<u64>)> = repo
        .lanes
        .iter()
        .map(|l| (l.lane.clone(), l.runs, l.median_wait_ms, l.waits_ms.clone()))
        .collect();
    assert_eq!(
        lanes,
        vec![
            ("mac".to_string(), 3, 5_000, vec![1_000, 9_000, 5_000]),
            ("android".to_string(), 1, 40_000, vec![40_000])
        ]
    );
}

#[test]
fn only_the_latest_waits_get_a_bar() {
    let scratch = Scratch::new("timing-bars");
    let path = store(&scratch);
    for n in 0..(WAITS_SHOWN as i64 + 5) {
        record(
            &path,
            &run(
                NOW - 1000 + n,
                Lane::MAC,
                n as u64,
                RunEnd::Passed,
                Vec::new(),
            ),
        )
        .unwrap();
    }
    let lane = &stats(&of_bae(&path), NOW - WINDOW_SECONDS).unwrap()[0].lanes[0];
    assert_eq!(lane.runs, WAITS_SHOWN as u32 + 5);
    assert_eq!(lane.waits_ms.len(), WAITS_SHOWN);
    assert_eq!(lane.waits_ms[0], 5);
    assert_eq!(*lane.waits_ms.last().unwrap(), WAITS_SHOWN as u64 + 4);
}

#[test]
fn a_store_with_no_run_in_the_window_has_no_figures_and_a_missing_file_has_none() {
    let scratch = Scratch::new("timing-empty");
    let path = store(&scratch);
    record(
        &path,
        &run(
            NOW - WINDOW_SECONDS - 1,
            Lane::MAC,
            0,
            RunEnd::Passed,
            Vec::new(),
        ),
    )
    .unwrap();
    assert!(
        stats(&of_bae(&path), NOW - WINDOW_SECONDS)
            .unwrap()
            .is_empty()
    );
    assert!(
        stats(&of_bae(&scratch.dir.join("none.sqlite")), 0)
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
            Lane::ANDROID,
            3_000,
            RunEnd::Passed,
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
    let json = serde_json::to_value(dashboard_figures(&of_bae(&path), NOW)).unwrap();
    assert_eq!(json["window_seconds"], WINDOW_SECONDS);
    let repo = &json["repos"][0];
    assert_eq!(repo["repo"], "bae");
    let lane = &repo["lanes"][0];
    assert_eq!(lane["lane"], "android");
    assert_eq!(lane["median_wait_ms"], 3_000);
    assert_eq!(lane["waits_ms"][0], 3_000);
    assert_eq!(lane["steps"][0]["name"], "android");
    assert_eq!(lane["steps"][0]["kind"], "step");
    assert_eq!(lane["steps"][0]["total_ms"], 7_000);
}

/// A repo two projects hold records each build in the store of the project
/// the claim of the branch's worktree names, else the one `--project` names,
/// and is refused with neither.
#[test]
fn a_build_of_a_two_project_repo_records_in_the_project_its_claim_or_flag_names() {
    let scratch = Scratch::new("timing-recording-project");
    let places = scratch.places();
    assert_eq!(
        recording_project(&places, "demo", "work", None).unwrap(),
        None
    );
    let clone = places.home.join(crate::host::clone_path("demo"));
    std::fs::create_dir_all(clone.parent().unwrap()).unwrap();
    crate::testing::repo(&clone, "target 1\n");
    let side = places.home.join("dev/demo-worktrees/side-1");
    let path = side.display().to_string();
    crate::testing::git(&clone, &["worktree", "add", "-q", "-b", "work", &path]);
    let commit = crate::testing::git_out(&clone, &["rev-parse", "HEAD"]);
    crate::lists::fixtures::make_store_named(&places, "first", &["demo"]);
    let of = |git_ref: &str, flag| recording_project(&places, "demo", git_ref, flag);
    assert_eq!(of(&commit, None).unwrap(), Some("first".to_string()));
    crate::lists::fixtures::make_store_named(&places, "second", &["demo"]);
    for git_ref in [commit.as_str(), "work"] {
        let refused = of(git_ref, None).unwrap_err().text();
        assert!(
            refused.contains("two projects (first, second)"),
            "{refused}"
        );
        assert_eq!(of(git_ref, Some("second")).unwrap(), Some("second".into()));
    }
    let refused = of(&commit, Some("third")).unwrap_err().text();
    assert!(refused.contains("does not hold repo demo"), "{refused}");
    let checkout = crate::checkout::Checkout::at(&side).unwrap().unwrap();
    crate::claim::set_project(&places, &checkout, Some("second")).unwrap();
    for git_ref in ["work", "refs/heads/work"] {
        assert_eq!(of(git_ref, None).unwrap(), Some("second".into()));
        assert_eq!(of(git_ref, Some("first")).unwrap(), Some("second".into()));
    }
    // The claim speaks for its branch, not for a commit.
    assert!(of(&commit, None).is_err());
}

/// The base-run lookup and the dashboard's figures of a repo two projects
/// hold read the runs of both stores, newest first.
#[test]
fn a_two_project_repos_runs_are_read_from_both_stores() {
    let scratch = Scratch::new("timing-two-stores");
    let places = scratch.places();
    let first = crate::lists::fixtures::make_store_named(&places, "first", &["bae"]);
    let second = crate::lists::fixtures::make_store_named(&places, "second", &["bae"]);
    let build = |at: i64, wait: u64, commit: &str, prefix: &str| {
        let name = format!("shots {prefix}");
        let shots = step(&name, StepKind::Shots, at, 1_000, StepResult::Passed);
        let mut run = run(at, Lane::MAC, wait, RunEnd::Passed, vec![shots]);
        record_checkout(&mut run, commit.repeat(40));
        run
    };
    record(&second.database(), &build(NOW - 20, 2_000, "a", "mac.a.")).unwrap();
    record(&first.database(), &build(NOW - 10, 4_000, "b", "mac.b.")).unwrap();
    let records = BuildRecords::open(&places, "bae").unwrap().unwrap();
    let ran = |commit: &str, prefix: &str| {
        records
            .has_passing_run(Lane::MAC, &commit.repeat(40), |run| run.ran_shots(prefix))
            .unwrap()
    };
    assert!(ran("a", "mac.a.") && ran("b", "mac.b.") && !ran("a", "mac.b."));
    let commits = ["a".repeat(40), "b".repeat(40)];
    let newest: Vec<i64> = records
        .passed_runs(Lane::MAC, None, &commits)
        .unwrap()
        .iter()
        .map(|run| run.started_at)
        .collect();
    assert_eq!(newest, [NOW - 10, NOW - 20]);
    let repos = [RepoDatabases {
        repo: "bae".to_string(),
        paths: vec![first.database(), second.database()],
    }];
    let mac = &stats(&repos, NOW - WINDOW_SECONDS).unwrap()[0].lanes[0];
    assert_eq!((mac.runs, &mac.waits_ms[..]), (2, &[2_000, 4_000][..]));
    let dashboard = crate::hook_stats::dashboard_json_at(&places, NOW);
    let dashboard: serde_json::Value = serde_json::from_str(&dashboard).unwrap();
    for project in dashboard["projects"].as_array().unwrap() {
        let lane = &project["app_builds"]["repos"][0]["lanes"][0];
        assert_eq!(lane["runs"], 2, "{project}");
    }
}

/// A run reads back as it was recorded: its load, the commit it built, and
/// each step with its result and what a failed or timed-out step said.
#[test]
fn a_recorded_run_reads_back_whole() {
    let scratch = Scratch::new("timing-read-back");
    let path = store(&scratch);
    let said = |excerpt: &str, shots: &[&str]| {
        crate::step_failure::StepFailure::new(
            Some(excerpt.to_string()),
            shots.iter().map(|shot| shot.to_string()),
        )
    };
    let mut built = run(
        NOW,
        Lane::ANDROID,
        3_000,
        RunEnd::Failed,
        vec![
            step("android", StepKind::Step, NOW, 9_000, StepResult::Passed),
            step(
                "shots android.a.",
                StepKind::Shots,
                NOW + 9,
                4_000,
                StepResult::Failed(said("2 pairs failed", &["android.a.one"])),
            ),
            step(
                "test Foo",
                StepKind::Test,
                NOW + 13,
                600_000,
                StepResult::TimedOut(said("hung", &[])),
            ),
            step(
                "android",
                StepKind::Step,
                NOW + 20,
                10,
                StepResult::TakenOver,
            ),
            step("android", StepKind::Step, NOW + 30, 10, StepResult::Stopped),
        ],
    );
    record_checkout(
        &mut built,
        "0123456789abcdef0123456789abcdef01234567".into(),
    );
    built.load = Some(crate::host::Load {
        cores: 8,
        one_minute: 2.5,
    });
    let bare = run(NOW + 60, Lane::MAC, 0, RunEnd::Stopped, Vec::new());
    record(&path, &built).unwrap();
    assert_eq!(crate::app_timing::read_named_run(&path).unwrap(), built);
    let other = scratch.dir.join("other.sqlite");
    drop(crate::db::open(&other, crate::db::Make::Allowed).unwrap());
    record(&other, &bare).unwrap();
    assert_eq!(crate::app_timing::read_named_run(&other).unwrap(), bare);
}

/// A failed step whose failure row the store lacks is a damaged store: reading
/// the run fails and names the step, and records no empty failure.
#[test]
fn a_failed_step_with_no_failure_row_fails_the_read() {
    let scratch = Scratch::new("timing-read-damaged");
    let path = store(&scratch);
    let failed = step(
        "android",
        StepKind::Step,
        NOW,
        9_000,
        StepResult::Failed(crate::step_failure::StepFailure::new(
            Some("boom".to_string()),
            Vec::<String>::new(),
        )),
    );
    record(
        &path,
        &run(NOW, Lane::ANDROID, 0, RunEnd::Failed, vec![failed]),
    )
    .unwrap();
    crate::app_build_step_failure::remove_every_failure(&path).unwrap();
    let said = crate::app_timing::read_named_run(&path).unwrap_err().text();
    assert!(said.contains("step android"), "{said}");
    assert!(said.contains("no failure row"), "{said}");
}

/// The shots prefixes a branch's app builds ran come back once each, whatever
/// the run's outcome and whether the step passed or failed. Runs match by
/// requested ref or built commit. Stopped steps, other lanes and repos, and
/// runs before the branch's first commit do not count.
#[test]
fn the_shots_prefixes_a_branch_ran_come_back_once_each_whatever_their_result() {
    let scratch = Scratch::new("timing-shots-ran");
    let path = store(&scratch);
    let shots = |name: &str, result: StepResult| {
        step(&format!("shots {name}"), StepKind::Shots, NOW, 1, result)
    };
    let mut first = run(
        NOW,
        Lane::MAC,
        0,
        RunEnd::Failed,
        vec![
            shots("mac.nowplaying.", StepResult::Failed(Default::default())),
            shots("mac.seek.", StepResult::Passed),
            step("macos", StepKind::Step, NOW, 1, StepResult::Passed),
        ],
    );
    record_checkout(&mut first, "a".repeat(40));
    record(&path, &first).unwrap();
    let mut second = run(
        NOW + 5,
        Lane::MAC,
        0,
        RunEnd::Passed,
        vec![
            shots("mac.seek.", StepResult::Passed),
            shots("mac.volume.", StepResult::Passed),
            shots("mac.stopped.", StepResult::Stopped),
        ],
    );
    record_checkout(&mut second, "b".repeat(40));
    second.built_ref = "tag".to_string();
    record(&path, &second).unwrap();
    let mut other_ref = run(
        NOW,
        Lane::MAC,
        0,
        RunEnd::Passed,
        vec![shots("mac.x.", StepResult::Passed)],
    );
    other_ref.built_ref = "other".to_string();
    record_checkout(&mut other_ref, "c".repeat(40));
    record(&path, &other_ref).unwrap();
    record(
        &path,
        &run(
            NOW,
            Lane::ANDROID,
            0,
            RunEnd::Passed,
            vec![shots("android.x.", StepResult::Passed)],
        ),
    )
    .unwrap();
    let mut other_repo = run(
        NOW,
        Lane::MAC,
        0,
        RunEnd::Passed,
        vec![shots("mac.y.", StepResult::Passed)],
    );
    other_repo.repo = "coven".to_string();
    record(&path, &other_repo).unwrap();
    record(
        &path,
        &run(
            NOW - 100,
            Lane::MAC,
            0,
            RunEnd::Passed,
            vec![shots("mac.old.", StepResult::Failed(Default::default()))],
        ),
    )
    .unwrap();
    let conn = crate::db::open_reader(&path).unwrap().unwrap();
    let commits = ["b".repeat(40)];
    assert_eq!(
        shots_ran(&conn, "bae", Lane::MAC, "branch", &commits, NOW, &path).unwrap(),
        ["mac.nowplaying.", "mac.seek.", "mac.volume."]
    );
    assert_eq!(
        shots_ran(
            &conn,
            "bae",
            Lane::MAC,
            "branch",
            &commits,
            NOW - 200,
            &path
        )
        .unwrap(),
        ["mac.nowplaying.", "mac.old.", "mac.seek.", "mac.volume."]
    );
}

#[test]
fn shots_ran_counts_a_run_named_by_a_branch_commit_that_recorded_no_commit() {
    let scratch = Scratch::new("timing-named-by-commit");
    let path = store(&scratch);
    let sha = "d".repeat(40);
    let mut failed = run(
        NOW,
        Lane::MAC,
        0,
        RunEnd::Failed,
        vec![step(
            "shots mac.base.",
            StepKind::Shots,
            NOW,
            1,
            StepResult::Failed(Default::default()),
        )],
    );
    failed.built_ref = sha.clone();
    failed.requested_commit = None;
    record(&path, &failed).unwrap();
    let conn = crate::db::open_reader(&path).unwrap().unwrap();
    assert_eq!(
        shots_ran(&conn, "bae", Lane::MAC, "branch", &[sha], NOW, &path).unwrap(),
        ["mac.base."]
    );
}

#[test]
fn a_step_keeps_whether_it_ran_at_the_merge_base_or_at_the_commit() {
    let scratch = Scratch::new("timing-at-merge-base");
    let path = store(&scratch);
    let at = |base| StepRecord {
        at_merge_base: Some(base),
        ..step("macos", StepKind::Step, NOW, 1, StepResult::Passed)
    };
    let recorded = run(NOW, Lane::MAC, 0, RunEnd::Passed, vec![at(true), at(false)]);
    record(&path, &recorded).unwrap();
    assert_eq!(crate::app_timing::read_named_run(&path).unwrap(), recorded);
}
