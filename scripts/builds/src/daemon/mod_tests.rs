use super::*;
use crate::testing::Scratch;
use std::sync::Arc;

fn model_in(scratch: &Scratch) -> Arc<Model> {
    let tool = scratch.tool(0);
    let places = tool.places.clone();
    let runs = Runs::start(places.clone(), vec!["/usr/bin/false".to_string()]).unwrap();
    Model::start(places, tool.rules.clone(), runs, tool.program.clone()).unwrap()
}

/// The reply `answer` writes for <question>.
fn ask(model: &Model, question: &Question) -> Reply {
    let (client, server) = UnixStream::pair().unwrap();
    writeln!(&client, "{}", serde_json::to_string(question).unwrap()).unwrap();
    answer(model, server).unwrap();
    let mut line = String::new();
    BufReader::new(client).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// The daemon judges its answer to a health question from the reading it
/// already holds. Reading the processes takes seconds on a loaded machine,
/// longer than a command waits, so the answer path never reads them: with
/// the volume gone, a reading made now would fail, and the daemon still
/// answers from the reading it holds.
#[test]
fn health_answers_from_the_reading_the_daemon_holds() {
    let scratch = Scratch::new("held-health");
    let model = model_in(&scratch);
    model.stop_sampling();
    model.sample_now().unwrap();
    let places = scratch.places();
    let away = scratch.dir.join("volume-away");
    std::fs::rename(&places.root, &away).unwrap();
    match ask(&model, &Question::Health) {
        Reply::Health(_) => {}
        Reply::Failed(message) => panic!("answered a failure: {message}"),
        Reply::Done => panic!("answered a note of writes to a health question"),
    }
    let why = model.sample_now().unwrap_err().to_string();
    match ask(&model, &Question::Health) {
        Reply::Failed(message) => assert!(message.contains(&why), "{message}"),
        Reply::Health(_) | Reply::Done => panic!("the daemon hid the failed sample"),
    }
    std::fs::rename(&away, &places.root).unwrap();
    model.sample_now().unwrap();
    assert!(matches!(ask(&model, &Question::Health), Reply::Health(_)));
}

/// A command that wrote build roots tells the daemon, which answers at once
/// and reads those roots again without watching the volume.
#[test]
fn a_note_of_what_a_command_wrote_is_answered_with_no_work() {
    let scratch = Scratch::new("wrote-note");
    let model = model_in(&scratch);
    let root = scratch.places().root.join("demo").join("side-1");
    match ask(&model, &Question::Wrote { roots: vec![root] }) {
        Reply::Done => {}
        Reply::Failed(message) => panic!("answered a failure: {message}"),
        Reply::Health(_) => panic!("answered health to a note of writes"),
    }
}

/// A claim written after the daemon's last reading of the processes reaches
/// the next health answer: `builds note --state held` and then `builds
/// health` see the hold before the daemon reads the processes again.
#[test]
fn a_health_answer_holds_a_claim_written_after_the_last_reading() {
    let scratch = Scratch::new("claim-after-reading");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "target 1G\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &side.display().to_string(),
        ],
    );
    let checkout = crate::checkout::Checkout::find(&side.display().to_string()).unwrap();
    crate::setup::setup(&tool, &crate::testing::no_ssh(), &checkout).unwrap();
    crate::claim::write_claim(&checkout, "agent A", tool.now, None).unwrap();
    let model = model_in(&scratch);
    // The daemon reads the worktree's git state on its own thread.
    let mut seen = 0;
    loop {
        let page = model.page().unwrap();
        let read = page.repos.iter().any(|repo| {
            repo.checkouts
                .iter()
                .any(|c| c.name == "side-1" && c.git.is_some())
        });
        if read {
            break;
        }
        seen = model.wait_change(seen);
    }
    // No reading of the processes comes after this one.
    model.stop_sampling();
    model.sample_now().unwrap();
    let note_state = |reply: Reply| match reply {
        Reply::Health(health) => {
            let worktree = health
                .worktrees
                .iter()
                .find(|w| w.checkout == "demo/side-1")
                .expect("health lists the claimed worktree");
            worktree.claim.state().map(str::to_string)
        }
        Reply::Failed(message) => panic!("answered a failure: {message}"),
        Reply::Done => panic!("answered a note of writes to a health question"),
    };
    assert_eq!(note_state(ask(&model, &Question::Health)), None);
    crate::claim::write_claim(
        &checkout,
        "agent A",
        tool.now,
        Some(crate::claim::Note::Held(
            "kept until the review".to_string(),
        )),
    )
    .unwrap();
    assert_eq!(
        note_state(ask(&model, &Question::Health)),
        Some("held".to_string()),
        "the answer missed the hold written after the last reading"
    );
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
    let _live = live::DashboardLock::new(&tool.places.daemon_lock())
        .claim()
        .unwrap()
        .unwrap();
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
