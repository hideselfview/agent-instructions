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
    needs(
        &config(),
        &paths(changed),
        "result",
        "branch",
        &mut |lane| {
            Ok(runs
                .iter()
                .filter(|(own, _)| *own == lane)
                .map(|(_, run)| run.clone())
                .collect())
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
            newest: Some(("older".to_string(), paths(&["bae-macos/Card.swift"]))),
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
            newest: Some(("app-only".to_string(), Vec::new())),
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
        newest: Some(("0123456789abcdef".to_string(), many)),
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
    ];
    cmd_landing_build(&places, &args).unwrap();
}
