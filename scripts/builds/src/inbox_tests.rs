use super::*;

#[test]
fn a_socket_path_must_be_absolute() {
    assert!(Messaging::new("cc.sock", None).is_err());
}

#[test]
fn a_test_never_sends_to_a_socket_outside_its_scratch_folder() {
    let live = Messaging::new("/tmp/cc-socks/1.sock", None).unwrap();
    let scratch = crate::testing::Scratch::new("refuse-live-inbox");
    let refused = scratch.sessions().send(&live, "x").unwrap_err().text();
    assert!(
        refused.contains("never reaches a live session"),
        "{refused}"
    );
}

#[test]
fn a_codex_worker_routes_to_its_own_thread_and_to_its_coordinators_thread() {
    let worker = "00000000-0000-4000-8000-000000000001";
    let coordinator = "00000000-0000-4000-8000-000000000002";
    let session =
        crate::ack::agent::Session::of(None, Some(coordinator.into()), Some(worker.into()), None);
    let calls = Destinations::for_session(&session).unwrap().unwrap();
    assert_eq!(
        calls.at(Recipient::ThisThread).stored(),
        ("codex", worker, None)
    );
    assert_eq!(
        calls.at(Recipient::Coordinator).stored(),
        ("codex", coordinator, None)
    );
}
