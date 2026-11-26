use super::*;
use crate::lists::fixtures::{NOON, item, make_store, stamp};
use crate::testing::Scratch;

#[test]
fn the_dashboard_starts_a_backlog_item_and_refuses_one_that_is_not_in_it() {
    let scratch = Scratch::new("start-dashboard");
    let places = scratch.places();
    let store = make_store(&places, &["bae"]);
    let id = store
        .write(&stamp(NOON), "item add", |db| {
            let id = item(db, vec![]);
            crate::lists::item::move_to_backlog(db, &[(id, "later".to_string())], &stamp(NOON))?;
            Ok(id)
        })
        .unwrap();
    start_item(&places, NOON + 60, "demo", id.0).unwrap();
    assert!(store.load().unwrap().item(id).unwrap().backlog.is_none());
    let refused = start_item(&places, NOON + 90, "demo", id.0)
        .unwrap_err()
        .text();
    assert!(refused.contains("is not in the backlog"), "{refused}");
}
