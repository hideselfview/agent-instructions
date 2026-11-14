use super::*;
use crate::testing::Scratch;
use std::sync::Arc;

fn model_in(scratch: &Scratch) -> Arc<Model> {
    let tool = scratch.tool(0);
    let places = tool.places.clone();
    let root = std::fs::canonicalize(&places.root).unwrap();
    let tracker = Tracker::start(&root, &places.daemon_dir()).unwrap();
    let runs = Runs::start(places.clone(), vec!["/usr/bin/false".to_string()]).unwrap();
    Model::start(places, tool.rules.clone(), tracker, runs)
}

/// The reply `answer` writes for <question>.
fn ask(model: &Model, given: &Path, question: &Question) -> Reply {
    let (client, server) = UnixStream::pair().unwrap();
    writeln!(&client, "{}", serde_json::to_string(question).unwrap()).unwrap();
    answer(model, given, server).unwrap();
    let mut line = String::new();
    BufReader::new(client).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// A health question gets the reading the daemon already holds. Reading the
/// processes takes seconds on a loaded machine, longer than a command waits,
/// so the answer path never reads them: with the volume gone, a reading made
/// now would fail, and the held one still answers.
#[test]
fn health_answers_from_the_reading_the_daemon_holds() {
    let scratch = Scratch::new("held-health");
    let model = model_in(&scratch);
    model.sample_now().unwrap();
    let places = scratch.places();
    std::fs::rename(&places.root, scratch.dir.join("volume-away")).unwrap();
    match ask(&model, &places.root, &Question::Health) {
        Reply::Health(_) => {}
        Reply::Failed(message) => panic!("answered a failure: {message}"),
        Reply::Measure(_) => panic!("answered sizes to a health question"),
    }
}

/// A tool whose volume is a short path: a socket's path must fit in
/// SUN_LEN, which the scratch folder's does not.
struct ShortVolume(PathBuf);

impl ShortVolume {
    fn new(name: &str) -> (ShortVolume, Tool) {
        let root = PathBuf::from(format!("/tmp/bt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut tool = Scratch::new(name).tool(0);
        tool.places.root = root.clone();
        (ShortVolume(root), tool)
    }
}

impl Drop for ShortVolume {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A socket file with no listener behind it, as a daemon leaves it after it
/// exits or a full backlog makes it look.
fn refused_socket(socket: &Path) {
    crate::testing::stale_socket(socket);
    assert_eq!(
        UnixStream::connect(socket).unwrap_err().kind(),
        std::io::ErrorKind::ConnectionRefused
    );
}

/// A new daemon never removes the socket of a live one, whatever connecting
/// to it says: macOS refuses a connection to a live daemon whose backlog is
/// full the same way it refuses one to a stale socket.
#[test]
fn bind_leaves_the_socket_of_a_live_daemon() {
    let (_volume, tool) = ShortVolume::new("live");
    std::fs::create_dir_all(tool.places.daemon_dir()).unwrap();
    let socket = tool.places.daemon_socket();
    refused_socket(&socket);
    let _live = live::claim(&tool.places.daemon_lock()).unwrap().unwrap();
    assert!(bind(&tool).is_err());
    assert!(socket.exists(), "the live daemon's socket was removed");
}

/// With no live daemon, bind replaces the socket one left behind.
#[test]
fn bind_replaces_a_stale_socket() {
    let (_volume, tool) = ShortVolume::new("stale");
    std::fs::create_dir_all(tool.places.daemon_dir()).unwrap();
    refused_socket(&tool.places.daemon_socket());
    let (_listener, _claim) = bind(&tool).unwrap();
    assert!(UnixStream::connect(tool.places.daemon_socket()).is_ok());
}
