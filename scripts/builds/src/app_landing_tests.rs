use super::*;
use crate::app_timing::PassedRun;

/// A repo whose mac app builds from bae-macos/, whose mac shots build from
/// bae-shots/ too, and whose android app builds from bae-android/.
fn config() -> Config {
    parse_config(
        "step mac macos ./macos.sh\nstep mac macos-shots ./shots.sh\nstep android android ./android.sh\non bae-macos/ macos macos-shots\non bae-shots/ macos-shots\non bae-android/ android\n",
    )
    .unwrap()
}

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|path| path.to_string()).collect()
}

fn passed(commit: &str, steps: &[&str]) -> PassedRun {
    PassedRun {
        commit: commit.to_string(),
        steps: paths(steps),
    }
}

/// What the landing of <changed> needs, given each lane's passing runs,
/// newest first, and the paths where each commit differs from the result
/// and from the branch (a commit missing from a list is one git does not
/// have).
fn check(
    changed: &[&str],
    runs: &[(Lane, PassedRun)],
    from_result: &[(&str, &[&str])],
    from_branch: &[(&str, &[&str])],
) -> Vec<Need> {
    check_shots(changed, runs, &[], from_result, from_branch)
}

/// `check`, for a landing whose mac lane requires the shots prefixes of
/// <required>: those the branch's app builds ran, less the dropped ones.
fn check_shots(
    changed: &[&str],
    runs: &[(Lane, PassedRun)],
    required: &[&str],
    from_result: &[(&str, &[&str])],
    from_branch: &[(&str, &[&str])],
) -> Vec<Need> {
    needs(
        &config(),
        &paths(changed),
        ("result", "branch"),
        &mut |lane| {
            Ok(runs
                .iter()
                .filter(|(own, _)| *own == lane)
                .map(|(_, run)| run.clone())
                .collect())
        },
        &mut |lane| {
            Ok(if lane == Lane::Mac {
                paths(required)
            } else {
                Vec::new()
            })
        },
        &mut |commit, target| {
            let differs = match target {
                "result" => from_result,
                "branch" => from_branch,
                other => panic!("no commit {other} in the test"),
            };
            Ok(differs
                .iter()
                .find(|(known, _)| *known == commit)
                .map(|(_, list)| paths(list)))
        },
        &mut |_, _| {},
    )
    .unwrap()
}

const BOTH: &[&str] = &["macos", "macos-shots"];

/// A landing that changes no path an `on` line matches changes no lane's
/// apps, so it needs no app build.
#[test]
fn a_landing_that_changes_no_app_needs_no_app_build() {
    assert_eq!(check(&["docs/a.md"], &[], &[], &[]), []);
}

/// The branch's own passing build covers the result when main moved only
/// at paths the lane does not compile.
#[test]
fn a_build_of_the_branch_covers_a_result_that_differs_only_where_the_apps_compile_nothing() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("built", BOTH))],
            &[("built", &["docs/a.md", "bae-android/Main.kt"])],
            &[("built", &[])],
        ),
        []
    );
}

/// Another landing that changed the mac apps reached main after the branch's
/// build: no build covers the result, so the lane needs a build of it with
/// the steps, tests and shots of the branch's build.
#[test]
fn a_landing_that_crossed_another_one_that_changed_the_apps_needs_the_branchs_build_again() {
    let steps = [
        "macos",
        "macos-shots",
        "test CardTests RowTests",
        "shots mac.card.",
    ];
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("built", &steps))],
            &[("built", &["bae-shots/Label.swift", "docs/a.md"])],
            &[("built", &["docs/a.md"])],
        ),
        [Need::Build {
            lane: Lane::Mac,
            steps: paths(&steps),
        }]
    );
}

/// With no build of the branch either, the lane is unbuilt, named with the
/// newest build and the paths where it differs from the branch.
#[test]
fn a_lane_with_no_build_of_the_branch_is_unbuilt() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("older", BOTH))],
            &[("older", &["bae-macos/Card.swift", "bae-shots/Label.swift"])],
            &[("older", &["bae-macos/Card.swift"])],
        ),
        [Need::Unbuilt(Unbuilt {
            lane: Lane::Mac,
            newest: Some(Newest {
                commit: "older".to_string(),
                paths: paths(&["bae-macos/Card.swift"]),
                missing_shots: Vec::new(),
            }),
        })]
    );
}

/// A build that skipped a step the change needs covers nothing, and an older
/// build that passed every step and has the branch's files is the branch's
/// build.
#[test]
fn a_build_covers_a_lane_only_when_it_passed_every_step_the_change_needs() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("app-only", &["macos"]))],
            &[("app-only", &[])],
            &[("app-only", &[])],
        ),
        [Need::Unbuilt(Unbuilt {
            lane: Lane::Mac,
            newest: Some(Newest {
                commit: "app-only".to_string(),
                paths: Vec::new(),
                missing_shots: Vec::new(),
            }),
        })]
    );
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[
                (Lane::Mac, passed("app-only", &["macos"])),
                (Lane::Mac, passed("whole", BOTH)),
            ],
            &[
                ("app-only", &["bae-macos/Other.swift"]),
                ("whole", &["bae-macos/Other.swift"])
            ],
            &[("app-only", &[]), ("whole", &[])],
        ),
        [Need::Build {
            lane: Lane::Mac,
            steps: paths(BOTH),
        }]
    );
}

/// A lane with no passing build of a commit git has is unbuilt, and each
/// lane the landing changes is checked on its own.
#[test]
fn a_lane_with_no_build_git_has_is_unbuilt_beside_a_covered_one() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift", "bae-android/Main.kt"],
            &[
                (Lane::Mac, passed("built", BOTH)),
                (Lane::Android, passed("gone", &["android"])),
            ],
            &[("built", &[])],
            &[("built", &[])],
        ),
        [Need::Unbuilt(Unbuilt {
            lane: Lane::Android,
            newest: None,
        })]
    );
}

/// The build's command names the steps of every lane first, then a flag for
/// each test class, shots prefix and full set of shots.
#[test]
fn the_build_command_names_the_steps_then_each_test_class_and_shots_run() {
    let mac = paths(&[
        "macos",
        "test CardTests RowTests",
        "shots mac.card.",
        "shots mac.",
    ]);
    let android = paths(&["android", "test MainTest"]);
    assert_eq!(
        build_command(
            "bae",
            "abc",
            &[(Lane::Mac, &mac), (Lane::Android, &android)]
        ),
        "builds app-build bae abc macos android --test mac CardTests --test mac RowTests --shots mac mac.card. --all-shots mac --test android MainTest"
    );
}

/// The refusal names each lane with the newest build's differing paths, at
/// most ten, and what the agent does next.
#[test]
fn the_refusal_names_each_lane_the_paths_and_the_next_steps() {
    let many: Vec<String> = (0..12).map(|n| format!("bae-macos/F{n}.swift")).collect();
    let mac = Unbuilt {
        lane: Lane::Mac,
        newest: Some(Newest {
            commit: "0123456789abcdef".to_string(),
            paths: many,
            missing_shots: Vec::new(),
        }),
    };
    let android = Unbuilt {
        lane: Lane::Android,
        newest: None,
    };
    let said = refusal("bae", "/w/side-3", "fedcba9876543210", &[&mac, &android]);
    assert!(
        said.contains("the branch's commit fedcba987654 for these lanes"),
        "{said}"
    );
    assert!(said.contains("mac: the newest passing mac app build, of 0123456789ab, has other files than the branch at bae-macos/F0.swift"), "{said}");
    assert!(said.contains("bae-macos/F9.swift and 2 more"), "{said}");
    assert!(
        said.contains("android: the store records no passing android app build"),
        "{said}"
    );
    assert!(
        said.contains("in /w/side-3, run `builds app-build bae <branch>`"),
        "{said}"
    );
}

/// agent-instructions is in two projects and has no `.app-build`: its
/// landings need no app build, and the command does not ask which project's
/// store records its app builds.
#[test]
fn a_result_with_no_app_build_file_needs_no_build_in_a_repo_two_projects_hold() {
    let scratch = crate::testing::Scratch::new("app-landing-two-projects");
    let places = scratch.places();
    let dir = scratch.dir.join("bae");
    crate::testing::repo(&dir, "target 1\n");
    crate::lists::fixtures::make_store_named(&places, "demo", &["bae"]);
    crate::lists::fixtures::make_store_named(&places, "second", &["bae"]);
    let args = [
        dir.display().to_string(),
        "HEAD".into(),
        "HEAD".into(),
        "HEAD".into(),
        "work".into(),
    ];
    cmd_landing_build(&places, &args).unwrap();
}

/// A branch ran the shots of two screens and the first failed as worse than
/// its base. A later passing build that ran only the second covers neither
/// the result nor the branch: the first prefix has no passing run.
#[test]
fn a_build_that_skipped_a_shots_prefix_the_branch_ran_covers_nothing() {
    let run = passed("built", &["macos", "macos-shots", "shots mac.seek."]);
    let same = [("built", &[] as &[&str])];
    assert_eq!(
        check_shots(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, run)],
            &["mac.nowplaying.", "mac.seek."],
            &same,
            &same,
        ),
        [Need::Unbuilt(Unbuilt {
            lane: Lane::Mac,
            newest: Some(Newest {
                commit: "built".to_string(),
                paths: Vec::new(),
                missing_shots: paths(&["mac.nowplaying."]),
            }),
        })]
    );
}

/// An older passing build that ran every prefix the branch ran is the
/// branch's build, and a run of a prefix that the requirement starts with
/// (the whole lane's `mac.`) covers it.
#[test]
fn a_build_that_ran_every_shots_prefix_the_branch_ran_covers_it() {
    let seek_only = passed("newer", &["macos", "macos-shots", "shots mac.seek."]);
    let both = passed(
        "older",
        &[
            "macos",
            "macos-shots",
            "shots mac.nowplaying.",
            "shots mac.seek.",
        ],
    );
    let all = passed("all", &["macos", "macos-shots", "shots mac."]);
    let same = [("newer", &[] as &[&str]), ("older", &[]), ("all", &[])];
    let required = ["mac.nowplaying.", "mac.seek."];
    assert_eq!(
        check_shots(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, seek_only.clone()), (Lane::Mac, both)],
            &required,
            &[
                ("newer", &["bae-macos/Other.swift"]),
                ("older", &["bae-macos/Other.swift"])
            ],
            &same,
        ),
        [Need::Build {
            lane: Lane::Mac,
            steps: paths(&[
                "macos",
                "macos-shots",
                "shots mac.nowplaying.",
                "shots mac.seek."
            ]),
        }]
    );
    assert_eq!(
        check_shots(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, seek_only), (Lane::Mac, all)],
            &required,
            &same,
            &same,
        ),
        []
    );
}

/// Land splits the lines about dropped prefixes from the build command.
#[test]
fn the_output_splits_the_dropped_lines_from_the_rest() {
    let drops = [Drop {
        prefix: "mac.seek.".into(),
        by: "agent-1".into(),
        why: "the board changed".into(),
    }];
    let line = dropped_lines(Lane::Mac, &drops).remove(0);
    assert_eq!(
        line,
        "shots dropped: the mac shots prefix mac.seek. needs no run before the landing; agent-1 recorded why: the board changed"
    );
    let said = format!("{line}\nbuilds app-build bae abc macos");
    assert_eq!(
        split_output(&said),
        (
            vec![line.as_str()],
            "builds app-build bae abc macos".to_string()
        )
    );
    assert_eq!(split_output(""), (vec![], String::new()));
}

/// Land prints the pairs a covering run rendered that the base never did.
#[test]
fn land_prints_the_pairs_the_base_never_rendered() {
    let findings = "FAIL x\nbuilds shots: 2 pair(s) the base never rendered, with 3 finding(s) that fail nothing: mac.a.b, mac.a.c\nbuilds shots: no pair fails the run\n";
    let line = unrendered_line(Lane::Mac, "0123456789abcdef", "mac.a.", findings).unwrap();
    assert_eq!(
        line,
        "shots the base never rendered: the mac shots run mac.a. of 0123456789ab: builds shots: 2 pair(s) the base never rendered, with 3 finding(s) that fail nothing: mac.a.b, mac.a.c"
    );
    assert_eq!(
        split_output(&format!("{line}\nbuilds app-build bae abc macos")),
        (
            vec![line.as_str()],
            "builds app-build bae abc macos".to_string()
        )
    );
    assert_eq!(
        unrendered_line(
            Lane::Mac,
            "abc",
            "mac.a.",
            "builds shots: no pair fails the run\n"
        ),
        None
    );
}

/// The refusal for a missing shots prefix names it and the drop command.
#[test]
fn the_refusal_names_the_missing_shots_prefixes_and_how_to_drop_one() {
    let mac = Unbuilt {
        lane: Lane::Mac,
        newest: Some(Newest {
            commit: "0123456789abcdef".to_string(),
            paths: Vec::new(),
            missing_shots: paths(&["mac.nowplaying.", "mac.seek."]),
        }),
    };
    let said = refusal("bae", "/w/side-3", "fedcba9876543210", &[&mac]);
    assert!(
        said.contains("mac: the newest passing mac app build, of 0123456789ab, did not run the shots prefixes mac.nowplaying., mac.seek. that the app builds of the branch ran since its merge base"),
        "{said}"
    );
    assert!(
        said.contains("`builds app-build --drop-shots bae <branch> <lane> <prefix> --why TEXT`"),
        "{said}"
    );
    assert!(!said.contains("did not pass every step"), "{said}");
}

/// The whole command, on a real repo and store: the branch's first build ran
/// the shots of two screens and one failed; its second build passed only the
/// other screen's shots. Land is refused until a passing build runs the first
/// prefix or an agent records why the branch drops it.
#[test]
fn a_landing_whose_covering_build_skipped_a_failed_prefix_is_refused_until_it_is_dropped() {
    use crate::app_timing::{StepKind, StepRecord, StepResult};
    use crate::run_end::RunEnd;
    let scratch = crate::testing::Scratch::new("app-landing-shots-gate");
    let places = scratch.places();
    let dir = scratch.dir.join("bae");
    crate::testing::repo(&dir, "target 1\n");
    let write = |path: &str, text: &str| {
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(path), text).unwrap();
    };
    write(
        ".app-build",
        "step mac macos ./macos.sh\non bae-macos/ macos\n",
    );
    write("bae-macos/a.swift", "one\n");
    crate::testing::git(&dir, &["add", ".app-build", "bae-macos/a.swift"]);
    crate::testing::git(&dir, &["commit", "-q", "-m", "base"]);
    let before = crate::testing::git_out(&dir, &["rev-parse", "HEAD"]);
    crate::testing::git(&dir, &["checkout", "-q", "-b", "work"]);
    write("bae-macos/a.swift", "two\n");
    crate::testing::git(&dir, &["commit", "-q", "-a", "-m", "change"]);
    let head = crate::testing::git_out(&dir, &["rev-parse", "HEAD"]);
    let since: i64 = crate::testing::git_out(&dir, &["log", "-1", "--format=%at"])
        .parse()
        .unwrap();
    let store = crate::lists::fixtures::make_store_named(&places, "demo", &["bae"]);
    let record_run = |at: i64, outcome: RunEnd, steps: &[(&str, StepKind, StepResult)]| {
        let run = crate::app_timing::RunRecord {
            started_at: at,
            repo: "bae".to_string(),
            lane: Lane::Mac,
            checkout: "side-1".to_string(),
            built_ref: "work".to_string(),
            built_commit: Some(head.clone()),
            who: None,
            wait_ms: 0,
            outcome,
            load: None,
            steps: steps
                .iter()
                .map(|(name, kind, result)| StepRecord {
                    name: name.to_string(),
                    kind: *kind,
                    started_at: at,
                    wall_ms: 1,
                    cpu_ms: 1,
                    result: result.clone(),
                })
                .collect(),
        };
        crate::app_timing::record(&store.database(), &run).unwrap();
    };
    record_run(
        since + 1,
        RunEnd::Failed,
        &[
            ("macos", StepKind::Step, StepResult::Passed),
            (
                "shots mac.nowplaying.",
                StepKind::Shots,
                StepResult::Failed(Default::default()),
            ),
        ],
    );
    record_run(
        since + 2,
        RunEnd::Passed,
        &[
            ("macos", StepKind::Step, StepResult::Passed),
            ("shots mac.seek.", StepKind::Shots, StepResult::Passed),
        ],
    );
    let args = [
        dir.display().to_string(),
        before,
        head.clone(),
        head,
        "work".to_string(),
    ];
    assert!(cmd_landing_build(&places, &args).is_err());
    let stamp = crate::lists::fixtures::stamp(since + 3);
    store
        .write(&stamp, "drop shots", |db| {
            crate::app_shots_drop::record(
                db,
                ("bae", "work"),
                Lane::Mac,
                "mac.nowplaying.",
                "the board moved",
                &stamp,
            )
        })
        .unwrap();
    cmd_landing_build(&places, &args).unwrap();
}

/// A result that holds another landing's commits beside the branch's: the
/// other landing changes the android apps, so judged over the whole result
/// the branch needs an android build it never made, while judged over its own
/// commits it needs the mac build it has.
#[test]
fn a_result_of_a_batch_is_judged_over_the_commits_of_each_landing() {
    use crate::app_timing::{StepKind, StepRecord, StepResult};
    use crate::run_end::RunEnd;
    let scratch = crate::testing::Scratch::new("app-landing-own");
    let places = scratch.places();
    let dir = scratch.dir.join("bae");
    crate::testing::repo(&dir, "target 1\n");
    let write = |path: &str, text: &str| {
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(path), text).unwrap();
    };
    write(
        ".app-build",
        "step mac macos ./macos.sh\nstep android android ./android.sh\non bae-macos/ macos\non bae-android/ android\n",
    );
    write("bae-macos/a.swift", "one\n");
    write("bae-android/a.kt", "one\n");
    crate::testing::git(&dir, &["add", "."]);
    crate::testing::git(&dir, &["commit", "-q", "-m", "base"]);
    let before = crate::testing::git_out(&dir, &["rev-parse", "HEAD"]);
    crate::testing::git(&dir, &["checkout", "-q", "-b", "work"]);
    write("bae-macos/a.swift", "two\n");
    crate::testing::git(&dir, &["commit", "-q", "-a", "-m", "mac change"]);
    let branch = crate::testing::git_out(&dir, &["rev-parse", "HEAD"]);
    let since: i64 = crate::testing::git_out(&dir, &["log", "-1", "--format=%at"])
        .parse()
        .unwrap();
    write("bae-android/a.kt", "two\n");
    crate::testing::git(&dir, &["commit", "-q", "-a", "-m", "android change"]);
    let result = crate::testing::git_out(&dir, &["rev-parse", "HEAD"]);
    let store = crate::lists::fixtures::make_store_named(&places, "demo", &["bae"]);
    let run = crate::app_timing::RunRecord {
        started_at: since + 1,
        repo: "bae".to_string(),
        lane: Lane::Mac,
        checkout: "side-1".to_string(),
        built_ref: "work".to_string(),
        built_commit: Some(branch.clone()),
        who: None,
        wait_ms: 0,
        outcome: RunEnd::Passed,
        load: None,
        steps: vec![StepRecord {
            name: "macos".to_string(),
            kind: StepKind::Step,
            started_at: since + 1,
            wall_ms: 1,
            cpu_ms: 1,
            result: StepResult::Passed,
        }],
    };
    crate::app_timing::record(&store.database(), &run).unwrap();
    let mut args = vec![
        dir.display().to_string(),
        before,
        result,
        branch.clone(),
        "work".to_string(),
    ];
    assert!(cmd_landing_build(&places, &args).is_err());
    args.push(branch);
    cmd_landing_build(&places, &args).unwrap();
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

#[test]
fn merged_builds_run_every_step_once_and_every_pick_once() {
    let merged = merge_builds(&[
        words("bae r1 macos --test mac CardTests --shots mac card."),
        words(
            "bae r1 macos macos-shots --test mac CardTests --test mac RowTests --all-shots android",
        ),
        words("bae r1 macos --all-shots android"),
    ])
    .unwrap();
    assert_eq!(
        merged,
        words(
            "bae r1 macos macos-shots --test mac CardTests --shots mac card. --test mac RowTests --all-shots android"
        )
    );
}

#[test]
fn one_build_merges_to_itself() {
    let one = words("bae r1 macos --test mac CardTests");
    assert_eq!(merge_builds(std::slice::from_ref(&one)).unwrap(), one);
}
