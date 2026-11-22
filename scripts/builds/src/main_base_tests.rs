use super::*;
use crate::app_config::PickKind;
use crate::shots_cache;
use crate::testing::Scratch;

fn key(commit: &str, prefix: &str, lane: Lane) -> shots_cache::Key {
    shots_cache::Key::new(
        "demo",
        commit,
        lane,
        prefix,
        &crate::shots_program::VERSIONS,
    )
}

/// Keeps a run of <key> whose report holds a findings.json.
fn cache_run(places: &Places, key: &shots_cache::Key) {
    let staging = shots_cache::stage(places, key).unwrap();
    std::fs::create_dir_all(staging.report()).unwrap();
    std::fs::write(staging.report().join(crate::shot_causes::FILE), "{}").unwrap();
    staging.finish(1, "FAIL x: y\n").unwrap();
}

fn screens() -> Vec<Pick> {
    vec![(Lane::Mac, PickKind::Shots, "mac.a.".to_string())]
}

const OLD: &str = "1111111111111111111111111111111111111111";
const TIP: &str = "2222222222222222222222222222222222222222";
const NOPROGRAM: &str = "/nonexistent/builds";

fn plan_of(places: &Places, base: &str, tip: &str) -> Result<Plan> {
    plan(places, "demo", base, tip, &screens(), Path::new(NOPROGRAM))
}

#[test]
fn a_cached_run_of_the_merge_base_needs_nothing() {
    let scratch = Scratch::new("main-base-covered");
    let places = scratch.places();
    cache_run(&places, &key(OLD, "mac.a.", Lane::Mac));
    assert_eq!(plan_of(&places, OLD, TIP).unwrap(), Plan::Covered);
}

#[test]
fn a_merge_base_with_no_run_makes_builds_run_mains_whole_lane_not_the_merge_base() {
    let scratch = Scratch::new("main-base-build");
    let places = scratch.places();
    assert_eq!(
        plan_of(&places, OLD, TIP).unwrap(),
        Plan::BuildMain(vec![(Lane::Mac, PickKind::Shots, "mac.".to_string())])
    );
    assert_eq!(
        plan_of(&places, TIP, TIP).unwrap(),
        Plan::BuildMain(vec![(Lane::Mac, PickKind::Shots, "mac.".to_string())])
    );
}

#[test]
fn a_branch_behind_a_cached_run_of_main_is_told_to_rebase_onto_main() {
    let scratch = Scratch::new("main-base-rebase");
    let places = scratch.places();
    cache_run(&places, &key(TIP, "mac.", Lane::Mac));
    assert_eq!(plan_of(&places, OLD, TIP).unwrap(), Plan::Rebase);
    // Once rebased, the whole-lane run covers the branch's prefix.
    assert_eq!(plan_of(&places, TIP, TIP).unwrap(), Plan::Covered);
}

#[test]
fn mains_run_of_one_lane_does_not_stand_in_for_another() {
    let scratch = Scratch::new("main-base-lanes");
    let places = scratch.places();
    cache_run(&places, &key(TIP, "mac.", Lane::Mac));
    let picks = vec![(Lane::Android, PickKind::Shots, "android.a.".to_string())];
    assert_eq!(
        plan(&places, "demo", OLD, TIP, &picks, Path::new(NOPROGRAM)).unwrap(),
        Plan::BuildMain(vec![(
            Lane::Android,
            PickKind::Shots,
            "android.".to_string()
        )])
    );
}

/// A branch's new shots (a prefix that no shot of main starts) still compare:
/// main's whole-lane run covers the prefix, so the branch's run takes its
/// shots and reports their scenes as ones the base never checked, and no
/// base run is refused for the prefix.
#[test]
fn a_prefix_that_matches_no_shot_of_main_is_covered_by_mains_whole_lane_run() {
    let scratch = Scratch::new("main-base-new-prefix");
    let places = scratch.places();
    let whole = key(TIP, "mac.", Lane::Mac);
    cache_run(&places, &whole);
    let picks = vec![(
        Lane::Mac,
        PickKind::Shots,
        "mac.component.heart.".to_string(),
    )];
    assert_eq!(
        plan(&places, "demo", TIP, TIP, &picks, Path::new(NOPROGRAM)).unwrap(),
        Plan::Covered
    );
    let before = shots_cache::before_run(
        &places,
        "demo",
        TIP,
        Lane::Mac,
        "mac.component.heart.",
        &crate::shots_program::VERSIONS,
    )
    .unwrap()
    .expect("the whole-lane run covers the new prefix");
    assert_eq!(before.key, whole);
}
