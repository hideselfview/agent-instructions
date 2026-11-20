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

/// The lanes the landing of <changed> leaves uncovered, given each lane's
/// passing runs, newest first, and the paths where each commit differs from
/// the result (a commit missing from <differs> is one git does not have).
fn check(
    changed: &[&str],
    runs: &[(Lane, PassedRun)],
    differs: &[(&str, &[&str])],
) -> Vec<Uncovered> {
    uncovered(
        &config(),
        &paths(changed),
        &mut |lane| {
            Ok(runs
                .iter()
                .filter(|(own, _)| *own == lane)
                .map(|(_, run)| run.clone())
                .collect())
        },
        &mut |commit| {
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
/// apps, so no app build has to cover it.
#[test]
fn a_landing_that_changes_no_app_needs_no_app_build() {
    assert_eq!(check(&["docs/a.md"], &[], &[]), []);
}

/// The branch's own passing build covers the landing when main moved only
/// at paths the lane does not compile.
#[test]
fn a_build_of_the_branch_covers_a_result_that_differs_only_where_the_apps_compile_nothing() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("built", BOTH))],
            &[("built", &["docs/a.md", "bae-android/Main.kt"])],
        ),
        []
    );
}

/// Another landing that changed the mac apps reached main after the branch's
/// build: the result differs from that build at the other landing's paths,
/// so no build covers the mac lane, and the refusal names those paths.
#[test]
fn a_landing_that_crossed_another_one_that_changed_the_apps_is_uncovered() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("built", BOTH))],
            &[("built", &["bae-shots/Label.swift", "docs/a.md"])],
        ),
        [Uncovered {
            lane: Lane::Mac,
            newest: Some(("built".to_string(), paths(&["bae-shots/Label.swift"]))),
        }]
    );
}

/// A build that skipped a step the change needs covers nothing, and an older
/// build that passed every step and has the result's files covers the lane.
#[test]
fn a_build_covers_a_lane_only_when_it_passed_every_step_the_change_needs() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift"],
            &[(Lane::Mac, passed("app-only", &["macos"]))],
            &[("app-only", &[])],
        ),
        [Uncovered {
            lane: Lane::Mac,
            newest: Some(("app-only".to_string(), Vec::new())),
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

/// A lane with no passing build of a commit git has is uncovered, and each
/// lane the landing changes is checked on its own.
#[test]
fn a_lane_with_no_build_git_has_is_uncovered_beside_a_covered_one() {
    assert_eq!(
        check(
            &["bae-macos/Card.swift", "bae-android/Main.kt"],
            &[
                (Lane::Mac, passed("built", BOTH)),
                (Lane::Android, passed("gone", &["android"])),
            ],
            &[("built", &[])],
        ),
        [Uncovered {
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
    let said = refusal(
        "bae",
        "/w/side-3",
        &[
            Uncovered {
                lane: Lane::Mac,
                newest: Some(("0123456789abcdef".to_string(), many)),
            },
            Uncovered {
                lane: Lane::Android,
                newest: None,
            },
        ],
    );
    assert!(said.contains("mac: the newest passing mac app build, of 0123456789ab, has other files than the result at bae-macos/F0.swift"), "{said}");
    assert!(said.contains("bae-macos/F9.swift and 2 more"), "{said}");
    assert!(
        said.contains("android: the store records no passing android app build"),
        "{said}"
    );
    assert!(
        said.contains("in /w/side-3, run `git fetch origin && git rebase origin/main`"),
        "{said}"
    );
    assert!(said.contains("`builds app-build bae <branch>`"), "{said}");
}
