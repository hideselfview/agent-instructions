use super::*;

#[test]
fn a_socket_path_must_be_absolute() {
    assert!(Messaging::new("cc.sock", None).is_err());
}

#[test]
fn a_test_never_sends_to_a_socket_outside_its_scratch_folder() {
    let live = Messaging::new("/tmp/cc-socks/1.sock", None).unwrap();
    let refused = send(&live, "x").unwrap_err().text();
    assert!(
        refused.contains("never reaches a live session"),
        "{refused}"
    );
}
