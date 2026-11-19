use super::*;

#[test]
fn a_socket_path_must_be_absolute() {
    assert!(Messaging::new("cc.sock", None).is_err());
}
