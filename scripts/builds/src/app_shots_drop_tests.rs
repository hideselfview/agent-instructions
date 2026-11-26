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
    drop_of(&store, "work", Lane::MAC, "mac.seek.", NOON).unwrap();
    drop_of(&store, "work", Lane::ANDROID, "android.seek.", NOON).unwrap();
    drop_of(&store, "other", Lane::MAC, "mac.x.", NOON).unwrap();
    drop_of(&store, "work", Lane::MAC, "mac.old.", SINCE - 1).unwrap();
    let db = store.open_existing().unwrap();
    assert_eq!(
        dropped(&db, "bae", "work", Lane::MAC, SINCE).unwrap(),
        [Drop {
            prefix: "mac.seek.".into(),
            by: "coordinator".into(),
            why: "the board moved".into(),
        }]
    );
    assert_eq!(dropped(&db, "bae", "gone", Lane::MAC, SINCE).unwrap(), []);
}

#[test]
fn a_web_lanes_build_and_dropped_prefix_are_recorded() {
    let scratch = Scratch::new("shots-drop-web");
    let store = make_store_named(&scratch.places(), "demo", &["bae"]);
    let run = RunRecord {
        started_at: NOON,
        repo: "bae".into(),
        lane: Lane::WEB,
        checkout: "side-1".into(),
        built_ref: "work".into(),
        requested_commit: None,
        built_commit: None,
        who: None,
        wait_ms: 0,
        outcome: RunEnd::Passed,
        load: None,
        steps: Vec::new(),
    };
    crate::app_timing::record(&store.database(), &run).unwrap();
    drop_of(&store, "work", Lane::WEB, "web.dashboard.", NOON).unwrap();
}

/// The lane checks of the runs' table and the drops' table take exactly the
/// app names.
#[test]
fn each_lane_column_takes_exactly_the_app_names() {
    let db = crate::db::memory();
    db.pragma_update(None, "foreign_keys", false).unwrap();
    let longest = "a".repeat(boards::app_build::APP_NAME_BYTES);
    let too_long = "a".repeat(boards::app_build::APP_NAME_BYTES + 1);
    for name in [
        "mac",
        "web",
        "zeta-shell-2",
        "7",
        &longest,
        &too_long,
        "",
        "-",
        "-web",
        "web-",
        "zeta--shell",
        "Web",
        "web.dashboard",
        "zeta_shell",
        "zeta shell",
        "wéb",
    ] {
        let run = db.execute(
            "INSERT INTO app_build_runs (started_at, repo, lane, checkout, built_ref, wait_ms, outcome) VALUES (1, 'bae', ?1, 'side-1', 'work', 0, 'passed')",
            [name],
        );
        let drop = db.execute(
            "INSERT INTO app_shots_drop (repo, branch, lane, prefix, at, by, why) VALUES ('bae', 'work', ?1, 'p.', 1, 'agent', 'why')",
            [name],
        );
        let takes = boards::app_build::is_app_name(name);
        assert_eq!(run.is_ok(), takes, "{name:?}");
        assert_eq!(drop.is_ok(), takes, "{name:?}");
    }
}

/// A drop with no reason is refused by the table.
#[test]
fn a_drop_with_no_reason_is_refused() {
    let scratch = Scratch::new("shots-drop-why");
    let store = make_store_named(&scratch.places(), "demo", &["bae"]);
    let stamp = stamp(NOON);
    let refused = store.write(&stamp, "drop shots", |db| {
        record(db, ("bae", "work"), Lane::MAC, "mac.seek.", " ", &stamp)
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
    let mut tool = scratch.tool(0);
    tool.dir = dir;
    let store = make_store_named(&tool.places, "demo", &["bae"]);
    let prefix = "mac.seek.";
    let run = RunRecord {
        started_at: tool.now,
        repo: "bae".into(),
        lane: Lane::MAC,
        checkout: "side-1".into(),
        built_ref: head.clone(),
        requested_commit: None,
        built_commit: Some(head.into()),
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
    cmd_drop_shots(&tool, &args).unwrap();
    let db = store.open_existing().unwrap();
    assert_eq!(
        dropped(&db, "bae", "work", Lane::MAC, tool.now).unwrap(),
        [Drop {
            prefix: prefix.into(),
            by: "agent".into(),
            why: "the board moved".into(),
        }]
    );
}
