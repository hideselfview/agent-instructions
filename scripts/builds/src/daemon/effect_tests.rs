use super::*;
use crate::lists::fixtures::{NOON, make_store, question, stamp};
use crate::testing::Scratch;

#[test]
fn the_dashboard_gives_an_open_question_the_effect_the_person_chose() {
    let scratch = Scratch::new("effect-dashboard");
    let places = scratch.places();
    let store = make_store(&places, &["bae"]);
    let id = store
        .write(&stamp(NOON), "question ask", |db| Ok(question(db)))
        .unwrap();
    set_question_effect(&places, NOON + 60, "demo", id.0, "data-loss").unwrap();
    assert_eq!(
        store.load().unwrap().question(id).unwrap().effect,
        Effect::DataLoss
    );
    let db = crate::db::open_reader(&store.database()).unwrap().unwrap();
    let logged = crate::lists::store::read_log(&db).unwrap().pop().unwrap();
    assert_eq!(
        (logged.2.as_str(), logged.3.as_str()),
        (
            super::super::approve::BY,
            "question edit 1 --effect data-loss"
        )
    );
}

#[test]
fn an_unknown_effect_or_question_is_refused() {
    let scratch = Scratch::new("effect-refused");
    let places = scratch.places();
    let store = make_store(&places, &["bae"]);
    let id = store
        .write(&stamp(NOON), "question ask", |db| Ok(question(db)))
        .unwrap();
    let refused = set_question_effect(&places, NOON, "demo", id.0, "worse")
        .unwrap_err()
        .text();
    assert!(
        refused.contains("the effect is data-loss, bug,"),
        "{refused}"
    );
    let refused = set_question_effect(&places, NOON, "demo", 9, "bug")
        .unwrap_err()
        .text();
    assert!(refused.contains("#9 is not an open question"), "{refused}");
}
