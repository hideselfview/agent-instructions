use super::*;
use crate::app_timing::{RunRecord, StepKind, StepRecord, StepResult};
use crate::lists::fixtures::{NOON, make_store_named, stamp};
use crate::run_end::RunEnd;
use crate::testing::{Scratch, git, git_out};

const SINCE: i64 = NOON - 100;

fn drop_of(store: &Store, branch: &str, lane: Lane, prefix: &str, at: i64) -> Result<()> {
    let stamp = stamp(at);
    store.write(&stamp, "drop shots", |db| {
        record(db, ("bae", branch), lane, prefix, "the board moved", &stamp)
    })
}

/// A drop names the branch, the lane and the prefix, and says who recorded it
/// and why; another branch, another lane and a drop older than the branch do
/// not count.
#[test]
fn a_drop_counts_for_its_branch_and_lane_since_the_branchs_first_commit() {
    let scratch = Scratch::new("shots-drop");
    let store = make_store_named(&scratch.places(), "demo", &["bae"]);
    drop_of(&store, "work", Lane::Mac, "mac.seek.", NOON).unwrap();
    drop_of(&store, "work", Lane::Android, "android.seek.", NOON).unwrap();
    drop_of(&store, "other", Lane::Mac, "mac.x.", NOON).unwrap();
    drop_of(&store, "work", Lane::Mac, "mac.old.", SINCE - 1).unwrap();
    let db = store.open_existing().unwrap();
    assert_eq!(
        dropped(&db, "bae", "work", Lane::Mac, SINCE).unwrap(),
        [Drop {
            prefix: "mac.seek.".into(),
            by: "coordinator".into(),
            why: "the board moved".into(),
        }]
    );
    assert_eq!(dropped(&db, "bae", "gone", Lane::Mac, SINCE).unwrap(), []);
}

/// A drop with no reason is refused by the table.
#[test]
fn a_drop_with_no_reason_is_refused() {
    let scratch = Scratch::new("shots-drop-why");
    let store = make_store_named(&scratch.places(), "demo", &["bae"]);
    let stamp = stamp(NOON);
    let refused = store.write(&stamp, "drop shots", |db| {
        record(db, ("bae", "work"), Lane::Mac, "mac.seek.", " ", &stamp)
    });
    assert!(refused.is_err());
}

#[test]
fn the_drop_command_accepts_a_prefix_from_a_build_named_by_commit() {
    let scratch = Scratch::new("shots-drop-commit");
    let dir = scratch.dir.join("bae");
    crate::testing::repo(&dir, "target 1\n");
    git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&dir, &["checkout", "-q", "-b", "work"]);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "change"]);
    let head = git_out(&dir, &["rev-parse", "HEAD"]);
    let tool = scratch.tool(0);
    let store = make_store_named(&tool.places, "demo", &["bae"]);
    let prefix = "mac.seek.";
    let run = RunRecord {
        started_at: tool.now,
        repo: "bae".into(),
        lane: Lane::Mac,
        checkout: "side-1".into(),
        built_ref: head.clone(),
        built_commit: Some(head),
        who: None,
        wait_ms: 0,
        outcome: RunEnd::Failed,
        load: None,
        steps: vec![StepRecord {
            name: format!("shots {prefix}"),
            kind: StepKind::Shots,
            started_at: tool.now,
            wall_ms: 1,
            cpu_ms: 1,
            at_merge_base: Some(false),
            result: StepResult::Failed(Default::default()),
            shots: None,
        }],
    };
    crate::app_timing::record(&store.database(), &run).unwrap();
    let args = [
        "bae",
        "work",
        "mac",
        prefix,
        "--why",
        "the board moved",
        "--by",
        "agent",
    ]
    .map(str::to_string);
    let original = current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();
    let result = cmd_drop_shots(&tool, &args);
    std::env::set_current_dir(original).unwrap();
    result.unwrap();
    let db = store.open_existing().unwrap();
    assert_eq!(
        dropped(&db, "bae", "work", Lane::Mac, tool.now).unwrap(),
        [Drop {
            prefix: prefix.into(),
            by: "agent".into(),
            why: "the board moved".into(),
        }]
    );
}
