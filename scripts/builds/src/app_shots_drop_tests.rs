use super::*;
use crate::lists::fixtures::{NOON, make_store_named, stamp};
use crate::testing::Scratch;

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
