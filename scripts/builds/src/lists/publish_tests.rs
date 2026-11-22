use super::*;
use crate::lists::design_command::run_in_session;
use crate::lists::fixtures::{NOON, stamp};
use crate::testing::{FakeInbox, Scratch, stale_socket};

const TOKEN: &str = "s3cret-token-9a8b";

/// Registers a design listener in a store that has the schema from before a
/// destination could name a Codex thread, as a Claude Code session did then.
pub(crate) fn plant_legacy_listener(
    db: &rusqlite::Connection,
    (socket, token): (&str, &str),
    (at, by): (i64, &str),
) {
    db.execute(
        "INSERT INTO design_listener (singleton, socket, token, at, by) VALUES (1, ?1, ?2, ?3, ?4)",
        rusqlite::params![socket, token, at, by],
    )
    .unwrap();
}
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
    let asked = ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
    .unwrap();
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
    let untaken = h.run(&["publish", "done", "1"]).unwrap_err().text();
    assert!(untaken.contains("was never taken"), "{untaken}");
    let taken = h.run(&["publish", "take", "1"]).unwrap();
    assert!(taken.contains(&format!("--at {COMMIT}")), "{taken}");
    let again = h.run(&["publish", "take", "1"]).unwrap_err().text();
    assert!(
        again.contains("design-session already took publish request 1"),
        "{again}"
    );
    let refused = ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
    .unwrap_err()
    .text();
    assert!(
        refused.contains("publish request 1 is still open"),
        "{refused}"
    );
    let none = h.run(&["publish", "done", "1"]).unwrap_err().text();
    assert!(none.contains("--link TITLE=URL"), "{none}");
    let bad = h
        .run(&["publish", "done", "1", "--link", "boards=ftp://x"])
        .unwrap_err()
        .text();
    assert!(bad.contains("no link"), "{bad}");
    assert_eq!(
        h.standing().latest.unwrap().ended,
        None,
        "a refused done ends nothing"
    );
    let done = h
        .run(&[
            "publish",
            "done",
            "1",
            "--link",
            "demo boards=https://claude.ai/artifact/AAA1",
            "--link",
            "Design system=https://claude.ai/artifact/DS1",
        ])
        .unwrap();
    assert!(
        done.contains(&format!("published commit {COMMIT}")),
        "{done}"
    );
    let twice = h.run(&["publish", "done", "1"]).unwrap_err().text();
    assert!(twice.contains("has ended"), "{twice}");
    let done = h.standing();
    let ended = done.latest.unwrap();
    assert_eq!(
        ended.ended,
        Some(Ended::Published {
            at: NOON + 60,
            links: vec![
                Link::parse("demo boards=https://claude.ai/artifact/AAA1").unwrap(),
                Link::parse("Design system=https://claude.ai/artifact/DS1").unwrap(),
            ],
        })
    );
    assert_eq!(ended.taken.unwrap().by, "design-session");
    let shown = serde_json::to_string(&h.standing()).unwrap();
    assert!(!shown.contains(TOKEN), "the token is never shown");
}

#[test]
fn with_no_session_listening_a_request_is_refused_or_ends_failed() {
    let h = held("publish-no-listener");
    let refused = ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
    .unwrap_err()
    .text();
    assert_eq!(refused, "No design session is listening for demo");
    assert_eq!(h.standing(), Standing::default(), "nothing was recorded");
    stale_socket(&h.socket());
    h.run(&["listen"]).unwrap();
    let gone = ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
    .unwrap_err()
    .text();
    assert_eq!(gone, "No design session is listening for demo");
    let latest = h.standing().latest.unwrap();
    assert_eq!(
        latest.ended,
        Some(Ended::Failed {
            at: NOON,
            error: "No design session is listening for demo".into()
        })
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
    let error = ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
    .unwrap_err()
    .text();
    assert!(
        error.starts_with("connecting to the session's inbox"),
        "{error}"
    );
    let ended = h.standing().latest.unwrap().ended.unwrap();
    assert_eq!(
        ended,
        Ended::Failed {
            at: NOON,
            error: error.clone()
        }
    );
    let again = ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
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
    ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
    .unwrap();
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
        latest.ended,
        Some(Ended::Failed {
            at: NOON + 60,
            error: "the canvas refused a call".into()
        })
    );
    let ended = h
        .run(&["publish", "failed", "1", "--why", "again"])
        .unwrap_err()
        .text();
    assert!(ended.contains("has ended"), "{ended}");
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
    assert!(outside.contains("Codex or Claude Code"), "{outside}");
    let unnamed = run_in_session(&tool, &args(&["listen", "--by", "d"]), || Ok(None))
        .unwrap_err()
        .text();
    assert!(unnamed.contains("--project"), "{unnamed}");
}

#[test]
fn a_queued_codex_publish_asks_for_resume_and_does_not_send_twice() {
    let h = held("publish-queued-codex");
    let count = h.scratch.dir.join("sends");
    h.scratch.script("codex", &format!("echo sent >> '{}'\necho 'Warning: queued the message but could not resume the thread' >&2", count.display()));
    let to = Messaging::restore("codex", "00000000-0000-4000-8000-000000000007", None).unwrap();
    h.store
        .write(&stamp(NOON), "design listen", |db| {
            listen(db, &to, &stamp(NOON))
        })
        .unwrap();

    let asked = ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
    .unwrap();
    assert_eq!(asked.delivery, Some(Delivery::Queued));
    assert!(asked.ended.is_none());
    assert_eq!(
        standing(&h.store.open_existing().unwrap())
            .unwrap()
            .latest
            .unwrap()
            .delivery,
        Some(Delivery::Queued)
    );
    assert!(
        ask_and_tell(
            &h.store,
            &h.scratch.places().sessions(),
            COMMIT,
            &stamp(NOON)
        )
        .is_err()
    );
    assert_eq!(std::fs::read_to_string(count).unwrap(), "sent\n");
}

#[test]
fn a_notify_write_failure_preserves_an_open_request_the_session_can_take() {
    let h = held("publish-notification-write");
    let inbox = FakeInbox::listen(&h.socket());
    h.run(&["listen"]).unwrap();
    let conn = crate::db::open(&h.store.database(), crate::db::Make::Never).unwrap();
    conn.execute_batch(
        "
        CREATE TRIGGER refuse_notification_update BEFORE UPDATE OF delivery ON publish_request
        WHEN NEW.delivery = 'delivered'
        BEGIN SELECT raise(ABORT, 'cannot record the delivery'); END;",
    )
    .unwrap();

    let failed = ask_and_tell(
        &h.store,
        &h.scratch.places().sessions(),
        COMMIT,
        &stamp(NOON),
    )
    .unwrap_err()
    .text();
    assert!(failed.contains("cannot record the delivery"), "{failed}");
    assert_eq!(inbox.received().len(), 2);
    let pending = h.standing().latest.unwrap();
    assert_eq!(pending.delivery, Some(Delivery::Unconfirmed));
    assert!(pending.ended.is_none());

    let taken = h
        .store
        .write(&stamp(NOON), "take publish", |db| {
            take(db, pending.id, &stamp(NOON))
        })
        .unwrap();
    assert!(taken.taken.is_some());
    assert_eq!(taken.delivery, None);
}

pub(crate) const PUBLISH_REQUEST: &str = r#"CREATE TABLE publish_request (
    id INTEGER PRIMARY KEY,
    -- The full id of the commit to publish.
    commit_id TEXT NOT NULL CONSTRAINT publish_request_commit_is_full CHECK (length(commit_id) = 40 AND commit_id NOT GLOB '*[^0-9a-f]*'),
    asked_at INTEGER NOT NULL,
    asked_by TEXT NOT NULL CONSTRAINT publish_request_asked_by_is_named CHECK (trim(asked_by) <> ''),
    taken_at INTEGER,
    taken_by TEXT CONSTRAINT publish_request_taken_by_is_named CHECK (taken_by IS NULL OR trim(taken_by) <> ''),
    ended_at INTEGER,
    outcome TEXT CONSTRAINT publish_request_outcome_known CHECK (outcome IS NULL OR outcome IN ('published', 'failed')),
    -- What stopped a failed request.
    error TEXT CONSTRAINT publish_request_error_says_something CHECK (error IS NULL OR trim(error) <> ''),
    CONSTRAINT publish_request_taken_whole CHECK ((taken_at IS NULL) = (taken_by IS NULL)),
    CONSTRAINT publish_request_ended_whole CHECK ((ended_at IS NULL) = (outcome IS NULL)),
    CONSTRAINT publish_request_failed_says_why CHECK ((outcome IS 'failed') = (error IS NOT NULL)),
    CONSTRAINT publish_request_published_was_taken CHECK (outcome IS NOT 'published' OR taken_at IS NOT NULL),
    -- Lets a link name the outcome it needs its request to have.
    CONSTRAINT publish_request_outcome_is_named UNIQUE (id, outcome)
) STRICT;"#;
