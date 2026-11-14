use super::*;
use crate::lists::design_command::run_in_session;
use crate::lists::fixtures::{NOON, stamp};
use crate::lists::project::NewProject;
use crate::testing::{FakeInbox, Scratch, git, repo, stale_socket};

const TOKEN: &str = "s3cret-token-1f2e";

/// A store holding open design change 1, in a scratch folder with a short
/// path for sockets.
struct Held {
    scratch: Scratch,
    store: Store,
}

fn held(name: &str) -> Held {
    let scratch = Scratch::new(name);
    let tool = scratch.tool(0);
    let store = crate::lists::fixtures::make_store(&tool.places, &["bae", "coven"]);
    store
        .write(&stamp(NOON), "design record", |db| {
            crate::lists::fixtures::design(db);
            Ok(())
        })
        .unwrap();
    Held { scratch, store }
}

impl Held {
    fn socket(&self) -> std::path::PathBuf {
        self.scratch.dir.join("s.sock")
    }

    fn keep(&self, messaging: Option<&Messaging>) {
        self.store
            .write(&stamp(NOON), "design record", |db| {
                keep(db, Id(1), messaging)
            })
            .unwrap();
    }

    fn state(&self) -> SessionState {
        self.store.load().unwrap().design(Id(1)).unwrap().session
    }
}

fn messaging(held: &Held, token: Option<&str>) -> Messaging {
    Messaging::new(held.socket().to_str().unwrap(), token).unwrap()
}

#[test]
fn approving_sends_the_auth_line_then_the_user_message_and_marks_the_session_told() {
    let h = held("session-told");
    let inbox = FakeInbox::listen(&h.socket());
    h.keep(Some(&messaging(&h, Some(TOKEN))));
    assert_eq!(h.state(), SessionState::Waiting);
    let approval = approve_and_tell(&h.store, Id(1), &stamp(NOON)).unwrap();
    assert_eq!(approval, Approval::Recorded(SessionState::Told));
    let lines = inbox.received();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let auth: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(auth, serde_json::json!({"type": "auth", "token": TOKEN}));
    let message: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(
        message,
        serde_json::json!({"type": "user", "message": {"role": "user", "content": "Design change #1 approved: land it."}})
    );
    assert_eq!(h.state(), SessionState::Told);
    let lists = h.store.load().unwrap();
    assert!(lists.design(Id(1)).unwrap().approved.is_some());
}

#[test]
fn a_session_with_no_token_gets_the_message_line_alone() {
    let h = held("session-no-token");
    let inbox = FakeInbox::listen(&h.socket());
    h.keep(Some(&messaging(&h, None)));
    approve_and_tell(&h.store, Id(1), &stamp(NOON)).unwrap();
    let lines = inbox.received();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("\"type\":\"user\""));
}

#[test]
fn a_gone_socket_keeps_the_approval_marks_the_session_ended_and_is_never_tried_again() {
    for gone in ["no file", "stale file"] {
        let h = held("session-gone");
        if gone == "stale file" {
            stale_socket(&h.socket());
        }
        h.keep(Some(&messaging(&h, Some(TOKEN))));
        let approval = approve_and_tell(&h.store, Id(1), &stamp(NOON)).unwrap();
        assert_eq!(approval, Approval::Recorded(SessionState::Ended), "{gone}");
        assert_eq!(h.state(), SessionState::Ended);
        assert!(
            h.store
                .load()
                .unwrap()
                .design(Id(1))
                .unwrap()
                .approved
                .is_some()
        );
        // The session coming back changes nothing: the approval is not sent again.
        let again = approve_and_tell(&h.store, Id(1), &stamp(NOON)).unwrap();
        assert_eq!(again, Approval::Already);
        assert_eq!(h.state(), SessionState::Ended);
    }
}

#[test]
fn a_change_with_no_recorded_session_is_approved_and_says_so() {
    let h = held("session-none");
    let approval = approve_and_tell(&h.store, Id(1), &stamp(NOON)).unwrap();
    assert_eq!(approval, Approval::Recorded(SessionState::Unrecorded));
    assert_eq!(h.state(), SessionState::Unrecorded);
}

#[test]
fn approving_twice_sends_once() {
    let h = held("session-twice");
    let inbox = FakeInbox::listen(&h.socket());
    h.keep(Some(&messaging(&h, Some(TOKEN))));
    approve_and_tell(&h.store, Id(1), &stamp(NOON)).unwrap();
    let again = approve_and_tell(&h.store, Id(1), &stamp(NOON)).unwrap();
    assert_eq!(again, Approval::Already);
    assert_eq!(inbox.received().len(), 2, "one auth line and one message");
}

#[test]
fn a_socket_path_must_be_absolute() {
    assert!(Messaging::new("cc.sock", None).is_err());
}

/// The token is kept in the lists store and nowhere a person or a page reads:
/// not in `Debug`, the lists JSON, `design show` or `design list`, the write
/// log, or an error.
#[test]
fn the_token_is_in_the_store_and_in_nothing_that_is_shown() {
    let h = held("session-secret");
    let inbox = FakeInbox::listen(&h.socket());
    let to = messaging(&h, Some(TOKEN));
    assert!(!format!("{to:?}").contains(TOKEN));
    h.keep(Some(&to));
    approve_and_tell(&h.store, Id(1), &stamp(NOON)).unwrap();
    inbox.received();

    let lists = h.store.load().unwrap();
    let json = serde_json::to_string(&lists.designs).unwrap();
    let view = serde_json::to_string(&crate::lists::view::design_view(
        &lists,
        &lists.designs[0],
        None,
    ))
    .unwrap();
    let show = crate::lists::design::show(&lists, Id(1)).unwrap();
    let list = crate::lists::design::list(&lists);
    for shown in [&json, &view, &show, &list] {
        assert!(
            !shown.contains(TOKEN) && !shown.contains(&h.socket().display().to_string()),
            "{shown}"
        );
    }
    let log = crate::lists::store::read_log(
        &crate::db::open_reader(&h.store.database())
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(!format!("{log:?}").contains(TOKEN));
    let bytes = std::fs::read(h.store.database()).unwrap();
    assert!(
        bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()),
        "the store keeps it, since the send needs it"
    );
    // A send that fails says what failed, not the token.
    let blocked = Messaging::new("/nonexistent-dir/\u{0}", Some(TOKEN));
    if let Ok(blocked) = blocked {
        let error = send(&blocked, "x").map(|_| ()).unwrap_err().text();
        assert!(!error.contains(TOKEN));
    }
}

/// Recording and amending keep the environment's inbox; an amend clears what
/// the session was told, and one made outside a session keeps the session.
#[test]
fn record_and_amend_keep_the_recording_sessions_inbox() {
    let scratch = Scratch::new("session-record");
    let dir = scratch.dir.join("bae");
    repo(&dir, "target 1\n");
    git(&dir, &["tag", "base"]);
    let tool = scratch.tool(0);
    let store = Store::named(&tool.places, "demo").unwrap();
    let globs = |g: &[&str]| {
        std::collections::BTreeMap::from([(
            "bae".to_string(),
            g.iter().map(|x| x.to_string()).collect(),
        )])
    };
    store
        .create(
            &NewProject {
                name: "demo".into(),
                repos: vec!["bae".into(), "coven".into()],
                places: globs(&["design/mocks/*.dc.html"]),
                sources: globs(&["design/system/tokens.css"]),
                specs: globs(&["spec/*.md"]),
            },
            &stamp(NOON),
            "lists init demo",
        )
        .unwrap();
    std::fs::create_dir_all(dir.join("design/system")).unwrap();
    std::fs::write(dir.join("design/system/tokens.css"), "a{}").unwrap();
    git(&dir, &["add", "design"]);
    git(&dir, &["commit", "-q", "-m", "tokens"]);
    let run = |session: Option<Messaging>, command: &str| {
        let mut args: Vec<String> = command.split(' ').map(String::from).collect();
        args.extend([
            "--by".into(),
            "design".into(),
            "--project".into(),
            "demo".into(),
        ]);
        run_in_session(&tool, &args, || Ok(session))
    };
    let request = |body: serde_json::Value| {
        let file = scratch.dir.join("request.json");
        std::fs::write(&file, body.to_string()).unwrap();
        file.display().to_string()
    };
    let body = serde_json::json!({
        "title": "Tokens", "details": "d", "worktree": dir.display().to_string(), "range": "base..HEAD",
        "no_app_place": "tokens only",
        "review": [{"heading": "Tokens", "words": "w"}],
    });
    let socket = scratch.dir.join("rec.sock");
    let recorded = Messaging::new(socket.to_str().unwrap(), Some(TOKEN)).unwrap();
    let printed = run(
        Some(recorded.clone()),
        &format!("record --request {}", request(body.clone())),
    )
    .unwrap();
    assert!(!printed.contains(TOKEN), "{printed}");
    assert_eq!(
        store.load().unwrap().designs[0].session,
        SessionState::Waiting
    );

    // Approved and told, then amended from outside a session: the person's
    // approval is cleared and the session still waits to be told.
    let inbox = FakeInbox::listen(&socket);
    approve_and_tell(&store, Id(1), &stamp(NOON)).unwrap();
    inbox.received();
    assert_eq!(store.load().unwrap().designs[0].session, SessionState::Told);
    let printed = run(None, &format!("amend 1 --request {}", request(body))).unwrap();
    assert!(!printed.contains(TOKEN), "{printed}");
    let after = store.load().unwrap();
    assert_eq!(after.designs[0].session, SessionState::Waiting);
    assert!(after.designs[0].approved.is_none());
}

/// The messaging environment is read only by `record` and `amend`: a
/// malformed inbox never fails another command.
#[test]
fn only_record_and_amend_read_the_messaging_environment() {
    let h = held("session-env");
    let tool = h.scratch.tool(0);
    let broken = || -> Result<Option<Messaging>> { fail!("the environment was read") };
    let args = |words: &[&str]| -> Vec<String> {
        words
            .iter()
            .map(|w| w.to_string())
            .chain(["--project".to_string(), "demo".to_string()])
            .collect()
    };
    assert!(run_in_session(&tool, &args(&["show", "1"]), broken).is_ok());
    assert!(run_in_session(&tool, &args(&["list"]), broken).is_ok());
    let refused = run_in_session(&tool, &args(&["record", "--request", "-"]), broken)
        .unwrap_err()
        .text();
    assert!(refused.contains("the environment was read"), "{refused}");
    assert!(Messaging::new("cc.sock", None).is_err());
}
