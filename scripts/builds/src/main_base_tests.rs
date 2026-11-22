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

fn pick(lane: Lane, prefix: &str) -> Pick {
    (lane, PickKind::Shots, prefix.to_string())
}

const OLD: &str = "1111111111111111111111111111111111111111";
const TIP: &str = "2222222222222222222222222222222222222222";
const NOPROGRAM: &str = "/nonexistent/builds";

/// The shots main's tip declares.
fn declared() -> Vec<String> {
    ["mac.a.one", "mac.a.two", "mac.b.one", "android.c.one"]
        .map(String::from)
        .to_vec()
}

fn plan_of(places: &Places, base: &str, picks: &[Pick]) -> Result<Plan> {
    plan(
        places,
        "demo",
        base,
        TIP,
        picks,
        || Ok(declared()),
        Path::new(NOPROGRAM),
    )
}

#[test]
fn a_cached_run_of_the_merge_base_needs_nothing() {
    let scratch = Scratch::new("main-base-covered");
    let places = scratch.places();
    cache_run(&places, &key(OLD, "mac.a.", Lane::Mac));
    assert_eq!(
        plan_of(&places, OLD, &[pick(Lane::Mac, "mac.a.")]).unwrap(),
        Plan::Covered
    );
}

#[test]
fn a_merge_base_with_no_run_makes_builds_run_only_the_asked_prefixes_at_mains_tip() {
    let scratch = Scratch::new("main-base-build");
    let places = scratch.places();
    let asked = [pick(Lane::Mac, "mac.a."), pick(Lane::Android, "android.c.")];
    assert_eq!(
        plan_of(&places, OLD, &asked).unwrap(),
        Plan::BuildMain(asked.to_vec())
    );
    assert_eq!(
        plan_of(&places, TIP, &asked).unwrap(),
        Plan::BuildMain(asked.to_vec())
    );
}

/// Two builds at one tip ask for overlapping prefixes: the second runs only
/// what the first's cached runs do not cover.
#[test]
fn builds_with_overlapping_prefixes_share_the_tips_cached_runs() {
    let scratch = Scratch::new("main-base-overlap");
    let places = scratch.places();
    let first = [pick(Lane::Mac, "mac.a."), pick(Lane::Mac, "mac.b.one")];
    assert_eq!(
        plan_of(&places, OLD, &first).unwrap(),
        Plan::BuildMain(first.to_vec())
    );
    // The first build's runs are cached.
    for prefix in ["mac.a.", "mac.b.one"] {
        cache_run(&places, &key(TIP, prefix, Lane::Mac));
    }
    // The second asks a narrower prefix of one and a new prefix: it runs only
    // the new one.
    let second = [pick(Lane::Mac, "mac.a.two"), pick(Lane::Mac, "mac.b.")];
    assert_eq!(
        plan_of(&places, OLD, &second).unwrap(),
        Plan::BuildMain(vec![pick(Lane::Mac, "mac.b.")])
    );
    // A build asking only the narrower prefix runs nothing: it rebases.
    assert_eq!(plan_of(&places, OLD, &second[..1]).unwrap(), Plan::Rebase);
    assert_eq!(plan_of(&places, TIP, &second[..1]).unwrap(), Plan::Covered);
}

#[test]
fn mains_run_of_one_lane_does_not_stand_in_for_another() {
    let scratch = Scratch::new("main-base-lanes");
    let places = scratch.places();
    cache_run(&places, &key(TIP, "mac.a.", Lane::Mac));
    let picks = [pick(Lane::Android, "android.c.")];
    assert_eq!(
        plan_of(&places, OLD, &picks).unwrap(),
        Plan::BuildMain(picks.to_vec())
    );
}

/// A branch's new shots (a prefix that no shot of main's tip starts) need no
/// run at the tip: the branch's run takes them, and every scene is new
/// against the base.
#[test]
fn a_prefix_that_matches_no_shot_at_the_tip_needs_no_tip_run() {
    let scratch = Scratch::new("main-base-new-prefix");
    let places = scratch.places();
    let picks = [pick(Lane::Mac, "mac.component.heart.")];
    assert_eq!(plan_of(&places, OLD, &picks).unwrap(), Plan::Covered);
    // Beside a prefix the tip does declare, only that one runs.
    let both = [picks[0].clone(), pick(Lane::Mac, "mac.a.")];
    assert_eq!(
        plan_of(&places, OLD, &both).unwrap(),
        Plan::BuildMain(vec![pick(Lane::Mac, "mac.a.")])
    );
}
