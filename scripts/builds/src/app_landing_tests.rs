use super::*;
use crate::app_timing::{PassedRun, StepKind, StepRecord, StepResult};
use crate::run_end::RunEnd;

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
        reused_shots: Vec::new(),
        commit: commit.to_string(),
        steps: paths(steps),
    }
}

/// The lanes the landing of <changed> leaves unbuilt, given each lane's
/// passing runs, newest first, and the paths where each commit differs from
/// the branch (a commit missing from the list is one git does not have).
fn check(
    changed: &[&str],
    runs: &[(Lane, PassedRun)],
    from_branch: &[(&str, &[&str])],
) -> Vec<Unbuilt> {
    check_shots(changed, runs, &[], from_branch)
}

/// `check`, for a landing whose mac lane requires the shots prefixes of
/// <required>: those the branch's app builds ran, less the dropped ones.
fn check_shots(
    changed: &[&str],
    runs: &[(Lane, PassedRun)],
    required: &[&str],
    from_branch: &[(&str, &[&str])],
) -> Vec<Unbuilt> {
    needs(
        &config(),
        &paths(changed),
        "branch",
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
            assert_eq!(target, "branch");
            Ok(from_branch
                .iter()
                .find(|(known, _)| *known == commit)
                .map(|(_, list)| paths(list)))
        },
        &mut |_, commit| panic!("these prefixes need no declarations at {commit}"),
    )
    .unwrap()
    .unbuilt
}

const BOTH: &[&str] = &["macos", "macos-shots"];

/// A landing that changes no path an `on` line matches changes no lane's
/// apps, so it needs no app build.
#[test]
fn a_landing_that_changes_no_app_needs_no_app_build() {
    assert_eq!(check(&["docs/a.md"], &[], &[]), []);
}

/// The branch's own passing build covers the landing however main moved: the
/// check never looks at main.
#[test]
fn a_build_of_the_branch_covers_it() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("built", BOTH))],
            &[("built", &[])],
        ),
        []
    );
}

/// With no build of the branch, the lane is unbuilt, named with the newest
/// build and the paths where it differs from the branch.
#[test]
fn a_lane_with_no_build_of_the_branch_is_unbuilt() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("older", BOTH))],
            &[("older", &["bae-macos/Card.swift"])],
        ),
        [Unbuilt {
            lane: Lane::Mac,
            newest: Some(Newest {
                commit: "older".to_string(),
                paths: paths(&["bae-macos/Card.swift"]),
                missing_shots: Vec::new(),
            }),
        }]
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
        ),
        [Unbuilt {
            lane: Lane::Mac,
            newest: Some(Newest {
                commit: "app-only".to_string(),
                paths: Vec::new(),
                missing_shots: Vec::new(),
            }),
        }]
    );
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[
                (Lane::Mac, passed("app-only", &["macos"])),
                (Lane::Mac, passed("whole", BOTH)),
            ],
            &[("app-only", &[]), ("whole", &[])],
        ),
        []
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
        ),
        [Unbuilt {
            lane: Lane::Android,
            newest: None,
        }]
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
fn a_branch_with_no_app_build_file_needs_no_build_in_a_repo_two_projects_hold() {
    let scratch = crate::testing::Scratch::new("app-landing-two-projects");
    let places = scratch.places();
    let dir = scratch.dir.join("bae");
    crate::testing::repo(&dir, "target 1\n");
    crate::lists::fixtures::make_store_named(&places, "demo", &["bae"]);
    crate::lists::fixtures::make_store_named(&places, "second", &["bae"]);
    let args = [dir.display().to_string(), "HEAD".into(), "work".into()];
    cmd_landing_coverage(&places, &args).unwrap();
}

/// A branch ran the shots of two screens and the first failed as worse than
/// its base. A later passing build that ran only the second covers nothing:
/// the first prefix has no passing run.
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
        ),
        [Unbuilt {
            lane: Lane::Mac,
            newest: Some(Newest {
                commit: "built".to_string(),
                paths: Vec::new(),
                missing_shots: paths(&["mac.nowplaying."]),
            }),
        }]
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
            &same,
        ),
        []
    );
    assert_eq!(
        check_shots(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, seek_only), (Lane::Mac, all)],
            &required,
            &same,
        ),
        []
    );
}

/// Land prints a line for each dropped prefix with who dropped it and why.
#[test]
fn a_dropped_prefix_is_printed_with_who_dropped_it_and_why() {
    let drops = [Drop {
        prefix: "mac.seek.".into(),
        by: "agent-1".into(),
        why: "the board changed".into(),
    }];
    assert_eq!(
        dropped_lines(Lane::Mac, &drops),
        [
            "shots dropped: the mac shots prefix mac.seek. needs no run before the landing; agent-1 recorded why: the board changed"
        ]
    );
}

/// Land prints the pairs a covering run rendered that the base never did.
#[test]
fn land_prints_the_pairs_the_base_never_rendered() {
    let findings = "FAIL x\nbuilds shots: 2 pair(s) the base never rendered, with 3 finding(s) that fail nothing: mac.a.b, mac.a.c\nbuilds shots: no pair fails the run\n";
    let line = unrendered_line(
        Lane::Mac,
        "0123456789abcdef",
        "26B131-17C529",
        "mac.a.",
        findings,
    )
    .unwrap();
    assert_eq!(
        line,
        "shots the base never rendered: the mac shots run mac.a. of 0123456789ab, made in environment 26B131-17C529: builds shots: 2 pair(s) the base never rendered, with 3 finding(s) that fail nothing: mac.a.b, mac.a.c"
    );
    assert_eq!(
        unrendered_line(
            Lane::Mac,
            "abc",
            "26B131-17C529",
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
        said.contains("mac: the newest passing mac app build, of 0123456789ab, did not cover the shots prefixes mac.nowplaying., mac.seek. that the app builds of the branch ran since its merge base"),
        "{said}"
    );
    assert!(
        said.contains("`builds app-build --drop-shots bae <branch> <lane> <prefix> --why TEXT`"),
        "{said}"
    );
    assert!(!said.contains("did not pass every step"), "{said}");
}

struct ShotsLanding {
    scratch: crate::testing::Scratch,
    store: crate::lists::store::Store,
    dir: std::path::PathBuf,
    head: String,
    since: i64,
}

const MAC_APP: &str =
    "step mac macos ./macos.sh\non bae-macos/ macos\nshots mac ./shots.sh {select}\n";

impl ShotsLanding {
    fn new() -> Self {
        let scratch = crate::testing::Scratch::new("app-landing-shots-gate");
        let dir = scratch.dir.join("bae");
        crate::testing::repo(&dir, "target 1\n");
        std::fs::create_dir(dir.join("bae-macos")).unwrap();
        std::fs::write(
            dir.join(".app-build"),
            format!("{MAC_APP}shots-declared mac bae-macos\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("bae-macos/Shots.swift"),
            "let scenes = [\"mac.nowplaying.empty\", \"mac.nowplaying.full\", \"mac.seek.knob\"]\n",
        )
        .unwrap();
        crate::testing::git(&dir, &["add", ".app-build", "bae-macos/Shots.swift"]);
        crate::testing::git(&dir, &["commit", "-q", "-m", "base"]);
        crate::testing::git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        crate::testing::git(&dir, &["checkout", "-q", "-b", "work"]);
        std::fs::write(dir.join("bae-macos/App.swift"), "let app = 1\n").unwrap();
        crate::testing::git(&dir, &["add", "bae-macos/App.swift"]);
        crate::testing::git(&dir, &["commit", "-q", "-m", "change"]);
        let head = crate::testing::git_out(&dir, &["rev-parse", "HEAD"]);
        let since = crate::testing::git_out(&dir, &["log", "-1", "--format=%at"])
            .parse()
            .unwrap();
        let store = crate::lists::fixtures::make_store_named(&scratch.places(), "demo", &["bae"]);
        Self {
            scratch,
            store,
            dir,
            head,
            since,
        }
    }

    fn record_run(&self, at: i64, outcome: RunEnd, shots: &[(&str, StepResult)]) {
        self.record_run_as("work", Some(&self.head), at, outcome, shots);
    }

    fn record_run_as(
        &self,
        built_ref: &str,
        built_commit: Option<&str>,
        at: i64,
        outcome: RunEnd,
        shots: &[(&str, StepResult)],
    ) {
        let steps = std::iter::once(("macos", StepKind::Step, StepResult::Passed)).chain(
            shots
                .iter()
                .map(|(name, result)| (*name, StepKind::Shots, result.clone())),
        );
        let run = crate::app_timing::RunRecord {
            reused_shots: Vec::new(),
            started_at: at,
            repo: "bae".to_string(),
            lane: Lane::Mac,
            checkout: "side-1".to_string(),
            built_ref: built_ref.to_string(),
            requested_commit: Some(self.head.clone()),
            built_commit: built_commit.map(str::to_string),
            who: None,
            wait_ms: 0,
            outcome,
            load: None,
            steps: steps
                .map(|(name, kind, result)| StepRecord {
                    at_merge_base: Some(built_commit.is_none()),
                    name: name.to_string(),
                    kind,
                    started_at: at,
                    wall_ms: 1,
                    cpu_ms: 1,
                    result,
                    shots: None,
                })
                .collect(),
        };
        crate::app_timing::record(&self.store.database(), &run).unwrap();
    }

    fn check(&self) -> Result<()> {
        let args = [
            self.dir.display().to_string(),
            self.head.clone(),
            "work".into(),
        ];
        cmd_landing_coverage(&self.scratch.places(), &args)
    }

    fn check_after_config_change(&self, config: &str) -> Result<()> {
        std::fs::write(self.dir.join(".app-build"), config).unwrap();
        crate::testing::git(&self.dir, &["add", ".app-build"]);
        crate::testing::git(
            &self.dir,
            &["commit", "-q", "-m", "Change app build config"],
        );
        let target = crate::testing::git_out(&self.dir, &["rev-parse", "HEAD"]);
        let args = [
            self.dir.display().to_string(),
            target.clone(),
            "work".into(),
            self.head.clone(),
            target,
        ];
        cmd_landing_coverage(&self.scratch.places(), &args)
    }
}

/// The branch ran two prefixes. Land refuses when the passing build skipped
/// one, and accepts it once an agent records why the branch drops that prefix.
#[test]
fn a_landing_whose_covering_build_skipped_a_failed_prefix_is_refused_until_it_is_dropped() {
    let landing = ShotsLanding::new();
    landing.record_run(
        landing.since + 1,
        RunEnd::Failed,
        &[(
            "shots mac.nowplaying.",
            StepResult::Failed(Default::default()),
        )],
    );
    landing.record_run(
        landing.since + 2,
        RunEnd::Passed,
        &[("shots mac.seek.", StepResult::Passed)],
    );
    assert!(landing.check().is_err());
    let stamp = crate::lists::fixtures::stamp(landing.since + 3);
    landing
        .store
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
    landing.check().unwrap();
}

#[test]
fn a_failed_shots_prefix_from_a_build_named_by_commit_is_required_at_landing() {
    for checked_out in [true, false] {
        let landing = ShotsLanding::new();
        landing.record_run_as(
            &landing.head,
            checked_out.then_some(landing.head.as_str()),
            landing.since + 1,
            RunEnd::Failed,
            &[(
                "shots mac.nowplaying.",
                StepResult::Failed(Default::default()),
            )],
        );
        landing.record_run(
            landing.since + 2,
            RunEnd::Passed,
            &[("shots mac.seek.", StepResult::Passed)],
        );
        assert!(
            matches!(landing.check(), Err(crate::fail::Fail::Exit(1))),
            "target commit checked out: {checked_out}"
        );
    }
}

#[test]
fn a_passing_build_covers_a_prefix_by_running_the_same_shots() {
    let landing = ShotsLanding::new();
    landing.record_run(
        landing.since + 1,
        RunEnd::Failed,
        &[(
            "shots mac.nowplaying",
            StepResult::Failed(Default::default()),
        )],
    );
    landing.record_run(
        landing.since + 2,
        RunEnd::Passed,
        &[("shots mac.nowplaying.", StepResult::Passed)],
    );
    landing.check().unwrap();
}

#[test]
fn one_passing_build_must_cover_every_shot_of_the_prefix() {
    let landing = ShotsLanding::new();
    landing.record_run(
        landing.since + 1,
        RunEnd::Failed,
        &[(
            "shots mac.nowplaying",
            StepResult::Failed(Default::default()),
        )],
    );
    landing.record_run(
        landing.since + 2,
        RunEnd::Passed,
        &[("shots mac.nowplaying.empty", StepResult::Passed)],
    );
    landing.record_run(
        landing.since + 3,
        RunEnd::Passed,
        &[("shots mac.nowplaying.full", StepResult::Passed)],
    );
    assert!(matches!(landing.check(), Err(crate::fail::Fail::Exit(1))));
    landing.record_run(
        landing.since + 4,
        RunEnd::Passed,
        &[
            ("shots mac.nowplaying.empty", StepResult::Passed),
            ("shots mac.nowplaying.full", StepResult::Passed),
        ],
    );
    landing.check().unwrap();
}

#[test]
fn an_older_build_can_cover_the_landing_when_declarations_are_unavailable() {
    let missing_folder = format!("{MAC_APP}shots-declared mac missing-mac\n");
    for config in [MAC_APP, &missing_folder] {
        let landing = ShotsLanding::new();
        landing.record_run(
            landing.since + 1,
            RunEnd::Passed,
            &[("shots mac.nowplaying", StepResult::Passed)],
        );
        landing.record_run(
            landing.since + 2,
            RunEnd::Passed,
            &[("shots mac.nowplaying.", StepResult::Passed)],
        );
        landing.check_after_config_change(config).unwrap();
    }
}

#[test]
fn mac_shots_coverage_reads_only_the_mac_harness() {
    for android_config in [
        "shots-declared android missing-android",
        "shots-declared android first\nshots-declared android second",
    ] {
        let landing = ShotsLanding::new();
        landing.record_run(
            landing.since + 1,
            RunEnd::Failed,
            &[(
                "shots mac.nowplaying",
                StepResult::Failed(Default::default()),
            )],
        );
        landing.record_run(
            landing.since + 2,
            RunEnd::Passed,
            &[("shots mac.nowplaying.", StepResult::Passed)],
        );
        landing
            .check_after_config_change(&format!(
                "{MAC_APP}shots-declared mac bae-macos\n{android_config}\n"
            ))
            .unwrap();
    }
}

/// A landing of the first of two commits on a branch is judged over its own
/// commits: the second changes the android apps, which the first landing
/// needs no build of, while a landing of both needs the android build the
/// branch never made.
#[test]
fn a_landing_is_judged_over_its_own_commits() {
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
    crate::testing::git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    crate::testing::git(&dir, &["checkout", "-q", "-b", "work"]);
    write("bae-macos/a.swift", "two\n");
    crate::testing::git(&dir, &["commit", "-q", "-a", "-m", "mac change"]);
    let first = crate::testing::git_out(&dir, &["rev-parse", "HEAD"]);
    let since: i64 = crate::testing::git_out(&dir, &["log", "-1", "--format=%at"])
        .parse()
        .unwrap();
    write("bae-android/a.kt", "two\n");
    crate::testing::git(&dir, &["commit", "-q", "-a", "-m", "android change"]);
    let second = crate::testing::git_out(&dir, &["rev-parse", "HEAD"]);
    let store = crate::lists::fixtures::make_store_named(&places, "demo", &["bae"]);
    let run = crate::app_timing::RunRecord {
        reused_shots: Vec::new(),
        started_at: since + 1,
        repo: "bae".to_string(),
        lane: Lane::Mac,
        checkout: "side-1".to_string(),
        built_ref: "work".to_string(),
        requested_commit: None,
        built_commit: Some(first.clone()),
        who: None,
        wait_ms: 0,
        outcome: RunEnd::Passed,
        load: None,
        steps: vec![StepRecord {
            at_merge_base: None,
            name: "macos".to_string(),
            kind: StepKind::Step,
            started_at: since + 1,
            wall_ms: 1,
            cpu_ms: 1,
            result: StepResult::Passed,
            shots: None,
        }],
    };
    crate::app_timing::record(&store.database(), &run).unwrap();
    let top = dir.display().to_string();
    let both = [
        top.clone(),
        second.clone(),
        "work".into(),
        first.clone(),
        second,
    ];
    assert!(cmd_landing_coverage(&places, &both).is_err());
    cmd_landing_coverage(&places, &[top, first, "work".into()]).unwrap();
}
