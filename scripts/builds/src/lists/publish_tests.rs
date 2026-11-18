use super::*;
use crate::lists::design_command::run_in_session;
use crate::lists::fixtures::{NOON, stamp};
use crate::testing::{FakeInbox, Scratch, stale_socket};

const TOKEN: &str = "s3cret-token-9a8b";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// A store of project demo, in a scratch folder with a short path for
/// sockets.
struct Held {
    scratch: Scratch,
    store: Store,
}

fn held(name: &str) -> Held {
    let scratch = Scratch::new(name);
    let tool = scratch.tool(0);
    let store = crate::lists::fixtures::make_store(&tool.places, &["bae"]);
    Held { scratch, store }
}

impl Held {
    fn socket(&self) -> std::path::PathBuf {
        self.scratch.dir.join("p.sock")
    }

    /// Runs `builds design <words> --project demo --by design-session` in a
    /// session whose inbox is this store's socket.
    fn run(&self, words: &[&str]) -> Result<String> {
        let mut tool = self.scratch.tool(0);
        tool.now = NOON + 60;
        let socket = self.socket();
        let args: Vec<String> = words
            .iter()
            .map(|w| w.to_string())
            .chain(["--project", "demo", "--by", "design-session"].map(String::from))
            .collect();
        run_in_session(&tool, &args, || {
            Messaging::new(socket.to_str().unwrap(), Some(TOKEN)).map(Some)
        })
    }

    fn standing(&self) -> Standing {
        standing(&self.store.open_existing().unwrap()).unwrap()
    }
}

#[test]
fn a_request_wakes_the_listening_session_which_takes_it_and_records_the_commit_it_published() {
    let h = held("publish-flow");
    let inbox = FakeInbox::listen(&h.socket());
    h.run(&["listen"]).unwrap();
    let asked = ask_and_tell(&h.store, COMMIT, &stamp(NOON)).unwrap();
    assert_eq!((asked.id, asked.commit.as_str()), (1, COMMIT));
    let lines = inbox.received();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains(TOKEN), "the auth line carries the token");
    let message: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(
        message["message"]["content"],
        serde_json::json!(asked_words("demo", 1, COMMIT))
    );
    let open = h.standing();
    assert_eq!(open.latest.as_ref().unwrap().taken, None);
    assert_eq!(open.published, None);
    let untaken = h.run(&["publish", "done", "1"]).unwrap_err().text();
    assert!(untaken.contains("was never taken"), "{untaken}");
    let taken = h.run(&["publish", "take", "1"]).unwrap();
    assert!(taken.contains(&format!("--at {COMMIT}")), "{taken}");
    let again = h.run(&["publish", "take", "1"]).unwrap_err().text();
    assert!(
        again.contains("design-session already took publish request 1"),
        "{again}"
    );
    let refused = ask_and_tell(&h.store, COMMIT, &stamp(NOON))
        .unwrap_err()
        .text();
    assert!(
        refused.contains("publish request 1 is still open"),
        "{refused}"
    );
    let done = h.run(&["publish", "done", "1"]).unwrap();
    assert!(
        done.contains(&format!("published commit {COMMIT}")),
        "{done}"
    );
    let twice = h.run(&["publish", "done", "1"]).unwrap_err().text();
    assert!(twice.contains("has ended"), "{twice}");
    let done = h.standing();
    assert_eq!(done.latest, done.published);
    let ended = done.published.unwrap();
    assert_eq!(
        ended.ended,
        Some(Ended {
            at: NOON + 60,
            error: None
        })
    );
    assert_eq!(ended.taken.unwrap().by, "design-session");
    let shown = serde_json::to_string(&h.standing()).unwrap();
    assert!(!shown.contains(TOKEN), "the token is never shown");
}

#[test]
fn with_no_session_listening_a_request_is_refused_or_ends_failed() {
    let h = held("publish-no-listener");
    let refused = ask_and_tell(&h.store, COMMIT, &stamp(NOON))
        .unwrap_err()
        .text();
    assert_eq!(refused, "No design session is listening for demo");
    assert_eq!(h.standing(), Standing::default(), "nothing was recorded");
    stale_socket(&h.socket());
    h.run(&["listen"]).unwrap();
    let gone = ask_and_tell(&h.store, COMMIT, &stamp(NOON))
        .unwrap_err()
        .text();
    assert_eq!(gone, "No design session is listening for demo");
    let latest = h.standing().latest.unwrap();
    assert_eq!(
        latest.ended.unwrap().error.as_deref(),
        Some("No design session is listening for demo")
    );
    assert_eq!(latest.taken, None);
}

#[test]
fn a_send_that_fails_other_than_by_the_session_being_gone_ends_the_request_failed() {
    let h = held("publish-send-fails");
    let blocked = h.scratch.dir.join("file");
    std::fs::write(&blocked, "").unwrap();
    let socket = blocked.join("p.sock");
    let mut tool = h.scratch.tool(0);
    tool.now = NOON + 60;
    let args: Vec<String> = ["listen", "--project", "demo", "--by", "design-session"]
        .map(String::from)
        .to_vec();
    run_in_session(&tool, &args, || {
        Messaging::new(socket.to_str().unwrap(), Some(TOKEN)).map(Some)
    })
    .unwrap();
    let error = ask_and_tell(&h.store, COMMIT, &stamp(NOON))
        .unwrap_err()
        .text();
    assert!(
        error.starts_with("connecting to the session's inbox"),
        "{error}"
    );
    let ended = h.standing().latest.unwrap().ended.unwrap();
    assert_eq!(ended.error.as_deref(), Some(error.as_str()));
    let again = ask_and_tell(&h.store, COMMIT, &stamp(NOON))
        .unwrap_err()
        .text();
    assert!(
        again.starts_with("connecting"),
        "a later ask is not refused as open: {again}"
    );
}

#[test]
fn a_failed_publish_ends_with_its_reason_and_a_new_listener_replaces_the_old() {
    let h = held("publish-failed");
    let inbox = FakeInbox::listen(&h.socket());
    h.run(&["listen"]).unwrap();
    h.run(&["listen"]).unwrap();
    ask_and_tell(&h.store, COMMIT, &stamp(NOON)).unwrap();
    assert_eq!(inbox.received().len(), 2, "one listener gets one message");
    h.run(&["publish", "take", "1"]).unwrap();
    let no_why = h.run(&["publish", "failed", "1"]).unwrap_err().text();
    assert!(no_why.contains("--why"), "{no_why}");
    h.run(&[
        "publish",
        "failed",
        "1",
        "--why",
        "the canvas refused a call",
    ])
    .unwrap();
    let latest = h.standing().latest.unwrap();
    assert_eq!(
        latest.ended.unwrap().error.as_deref(),
        Some("the canvas refused a call")
    );
    let ended = h
        .run(&["publish", "failed", "1", "--why", "again"])
        .unwrap_err()
        .text();
    assert!(ended.contains("has ended"), "{ended}");
    assert_eq!(h.standing().published, None);
}

#[test]
fn listen_needs_a_session_inbox_and_a_project() {
    let h = held("publish-listen");
    let tool = h.scratch.tool(0);
    let args = |words: &[&str]| -> Vec<String> { words.iter().map(|w| w.to_string()).collect() };
    let outside = run_in_session(
        &tool,
        &args(&["listen", "--project", "demo", "--by", "d"]),
        || Ok(None),
    )
    .unwrap_err()
    .text();
    assert!(
        outside.contains("outside a Claude Code session"),
        "{outside}"
    );
    let unnamed = run_in_session(&tool, &args(&["listen", "--by", "d"]), || Ok(None))
        .unwrap_err()
        .text();
    assert!(unnamed.contains("--project"), "{unnamed}");
}
