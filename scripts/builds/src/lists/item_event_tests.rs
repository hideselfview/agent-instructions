use super::*;
use crate::lists::fixtures::{NOON, db, item, item_opened, question, read, stamp};
use crate::lists::record::close;
use crate::lists::{How, Running, Wait};

fn kinds(db: &Connection, id: Id) -> Vec<(i64, Kind, Option<String>)> {
    read_item(db, id)
        .unwrap()
        .into_iter()
        .map(|event| (event.at, event.kind, event.detail))
        .collect()
}

fn follow_at(db: &Connection, at: i64) {
    follow(db, &read(db), at).unwrap();
}

#[test]
fn an_item_with_no_wait_is_ready_from_the_write_that_opened_it() {
    let db = db();
    let id = item(&db, vec![]);
    follow_at(&db, NOON + 5);
    assert_eq!(kinds(&db, id), vec![(NOON + 5, Kind::Ready, None)]);
    follow_at(&db, NOON + 9);
    assert_eq!(
        kinds(&db, id).len(),
        1,
        "a write that changes nothing adds nothing"
    );
}

#[test]
fn a_wait_is_gained_and_then_lost_when_the_record_it_waits_on_closes() {
    let db = db();
    let asked = question(&db);
    crate::lists::fixtures::blocks_something(&db, asked);
    let id = item(&db, vec![Wait::Question(asked)]);
    follow_at(&db, NOON + 10);
    let wait = format!("question:{}", asked.0);
    assert_eq!(
        kinds(&db, id),
        vec![
            (NOON + 10, Kind::WaitGained, Some(wait.clone())),
            (NOON + 10, Kind::Blocked, None),
        ]
    );
    close(&db, asked, How::Settled("rule".into()), &stamp(NOON + 20)).unwrap();
    follow_at(&db, NOON + 20);
    assert_eq!(
        kinds(&db, id)[2..],
        [
            (NOON + 20, Kind::WaitLost, Some(wait)),
            (NOON + 20, Kind::Ready, None),
        ]
    );
}

#[test]
fn removing_a_wait_loses_it_too() {
    let db = db();
    let asked = question(&db);
    crate::lists::fixtures::blocks_something(&db, asked);
    let id = item(&db, vec![Wait::Question(asked)]);
    follow_at(&db, NOON + 10);
    crate::lists::item::remove_wait(&db, id, &Wait::Question(asked)).unwrap();
    follow_at(&db, NOON + 15);
    let found = kinds(&db, id);
    assert_eq!(found[2].1, Kind::WaitLost);
    assert_eq!(found[3], (NOON + 15, Kind::Ready, None));
}

#[test]
fn taking_an_item_and_stopping_it_are_states_of_their_own() {
    let db = db();
    let id = item(&db, vec![]);
    follow_at(&db, NOON + 1);
    crate::lists::running::start(
        &db,
        &[id],
        &[Running {
            worktree: "/d/bae-worktrees/side-2".into(),
            branch: "work".into(),
            brief: "b.md".into(),
            since: stamp(NOON + 2),
            tier: crate::model_tier::Tier::Implementation,
        }],
    )
    .unwrap();
    follow_at(&db, NOON + 2);
    crate::lists::running::stop(&db, id, &stamp(NOON + 3)).unwrap();
    follow_at(&db, NOON + 3);
    let states: Vec<Kind> = kinds(&db, id)
        .into_iter()
        .map(|(_, kind, _)| kind)
        .collect();
    assert_eq!(states, vec![Kind::Ready, Kind::Running, Kind::Ready]);
}

#[test]
fn an_item_placed_last_behind_others_is_blocked_and_says_why() {
    let db = db();
    let other = item(&db, vec![]);
    let last = item(&db, vec![]);
    crate::lists::item::set_priority(
        &db,
        last,
        Some(crate::lists::Priority {
            level: crate::lists::Level::Last,
            why: "later".into(),
        }),
    )
    .unwrap();
    follow_at(&db, NOON + 1);
    assert_eq!(
        kinds(&db, last),
        vec![(NOON + 1, Kind::Blocked, Some("placed last".into()))]
    );
    assert_eq!(kinds(&db, other), vec![(NOON + 1, Kind::Ready, None)]);
}

#[test]
fn an_item_opened_before_the_store_recorded_events_has_none() {
    let db = db();
    let old = item_opened(&db, vec![], stamp(NOON));
    start_recording(&db, NOON + 100).unwrap();
    let new = item_opened(&db, vec![], stamp(NOON + 200));
    follow_at(&db, NOON + 200);
    assert_eq!(kinds(&db, old), vec![]);
    assert_eq!(kinds(&db, new), vec![(NOON + 200, Kind::Ready, None)]);
    assert_eq!(read_from(&db).unwrap(), Some(NOON + 100));
}

#[test]
fn every_write_of_the_store_records_the_events_of_its_own_time() {
    let scratch = crate::testing::Scratch::new("item-event-store");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["bae"]);
    let id = store
        .write(&stamp(NOON + 30), "add item", |db| Ok(item(db, vec![])))
        .unwrap();
    store
        .write(&stamp(NOON + 40), "handoff", |db| {
            record(db, id, NOON + 40, Kind::HandedOff, Some("b.md"))
        })
        .unwrap();
    let db = store.open_existing().unwrap();
    assert_eq!(
        kinds(&db, id),
        vec![
            (NOON + 30, Kind::Ready, None),
            (NOON + 40, Kind::HandedOff, Some("b.md".into())),
        ]
    );
}

#[test]
fn the_table_refuses_a_kind_it_does_not_know() {
    let db = db();
    let id = item(&db, vec![]);
    assert!(
        db.execute(
            "INSERT INTO item_event (item_id, at, kind) VALUES (?1, 1, 'late')",
            [id]
        )
        .is_err()
    );
}

#[test]
fn an_item_in_the_backlog_records_the_backlog_event_with_its_why_and_ready_on_start() {
    let db = db();
    let id = item(&db, vec![]);
    follow_at(&db, NOON + 5);
    crate::lists::item::move_to_backlog(&db, &[(id, "later".to_string())], &stamp(NOON + 10))
        .unwrap();
    follow_at(&db, NOON + 10);
    assert_eq!(
        kinds(&db, id)[1..],
        [(NOON + 10, Kind::Backlog, Some("later".to_string()))]
    );
    follow_at(&db, NOON + 11);
    assert_eq!(
        kinds(&db, id).len(),
        2,
        "a write that changes nothing adds nothing"
    );
    crate::lists::item::start_from_backlog(&db, &[id], "now", &stamp(NOON + 15)).unwrap();
    follow_at(&db, NOON + 20);
    assert_eq!(kinds(&db, id)[2..], [(NOON + 20, Kind::Ready, None)]);
}
