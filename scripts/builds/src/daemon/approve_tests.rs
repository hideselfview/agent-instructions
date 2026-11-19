use super::*;
use crate::inbox::Messaging;
use crate::lists::design_session::{SessionState, keep};
use crate::lists::fixtures::{NOON, design, make_store, stamp};
use crate::testing::{FakeInbox, Scratch};

#[test]
fn the_dashboard_approves_the_design_change_in_the_project_that_holds_it_and_tells_its_session() {
    let scratch = Scratch::new("approve-dashboard");
    let places = scratch.places();
    let store = make_store(&places, &["bae", "coven"]);
    let socket = scratch.dir.join("s.sock");
    store
        .write(&stamp(NOON), "design record", |db| {
            let id = design(db);
            keep(
                db,
                id,
                Some(&Messaging::new(socket.to_str().unwrap(), Some("tok")).unwrap()),
            )
        })
        .unwrap();
    let inbox = FakeInbox::listen(&socket);
    let approval = approve_design(&places, NOON + 60, "demo", 1).unwrap();
    assert_eq!(approval, Approval::Recorded(SessionState::Told));
    let lines = inbox.received();
    assert!(
        lines[1].contains("Design change #1 of project demo approved: land it."),
        "{lines:?}"
    );
    let lists = store.load().unwrap();
    let approved = lists.design(Id(1)).unwrap().approved.clone().unwrap();
    assert_eq!((approved.at, approved.by.as_str()), (NOON + 60, BY));
    assert_eq!(
        approve_design(&places, NOON + 90, "demo", 1).unwrap(),
        Approval::Already
    );
}

#[test]
fn a_design_change_no_project_has_is_refused() {
    let scratch = Scratch::new("approve-none");
    let places = scratch.places();
    make_store(&places, &["bae"]);
    let refused = approve_design(&places, NOON, "demo", 7).unwrap_err().text();
    assert!(
        refused.contains("project demo has no design change"),
        "{refused}"
    );
}
