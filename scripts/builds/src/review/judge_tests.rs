use super::*;
use crate::lists::fixtures::{NOON, make_store};
use crate::testing::Scratch;
use rusqlite::OptionalExtension;

const TOKEN: &str = "judge-token-7c1d";

/// A store of project demo, in a scratch folder with a short path for
/// sockets.
struct Held {
    scratch: Scratch,
    tool: Tool,
    store: Store,
}

fn held(name: &str) -> Held {
    let scratch = Scratch::new(name);
    let mut tool = scratch.tool(0);
    tool.now = NOON;
    let store = make_store(&tool.places, &["bae"]);
    Held {
        scratch,
        tool,
        store,
    }
}

fn caller(session: &str, id: &str) -> Caller {
    Caller {
        session: session.to_string(),
        id: id.to_string(),
    }
}

impl Held {
    fn inbox(&self) -> Messaging {
        Messaging::new(
            self.scratch.dir.join("j.sock").to_str().unwrap(),
            Some(TOKEN),
        )
        .unwrap()
    }

    /// Runs `builds judge listen --project demo --by judge` as <caller>.
    fn listen(&self, caller: Option<&Caller>, inbox: Option<&Messaging>) -> Result<String> {
        let args: Vec<String> = ["--project", "demo", "--by", "judge"]
            .map(String::from)
            .to_vec();
        listen_command(&self.tool, &args, &self.scratch.dir, caller, inbox)
    }

    /// The judge the store keeps: its socket, token, session and name.
    fn kept(&self) -> Option<(String, Option<String>, String, String)> {
        self.store
            .open_existing()
            .unwrap()
            .query_row(
                "SELECT socket, token, session, by FROM review_judge",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .unwrap()
    }
}

#[test]
fn the_session_that_listens_is_kept_as_the_judge_and_a_later_one_replaces_it() {
    let h = held("judge-listen");
    assert_eq!(h.kept(), None, "no session has listened");
    let said = h
        .listen(Some(&caller("s-old", "s-old")), Some(&h.inbox()))
        .unwrap();
    assert!(said.contains("project demo's judge"), "{said}");
    let socket = h.inbox().socket().to_string();
    assert_eq!(
        h.kept(),
        Some((
            socket.clone(),
            Some(TOKEN.to_string()),
            "s-old".to_string(),
            "judge".to_string()
        ))
    );
    h.listen(Some(&caller("s-new", "s-new")), Some(&h.inbox()))
        .unwrap();
    assert_eq!(h.kept().unwrap().2, "s-new");
}

#[test]
fn only_a_sessions_main_agent_with_an_inbox_listens() {
    let h = held("judge-refused");
    let dispatched = h
        .listen(Some(&caller("s1", "a1")), Some(&h.inbox()))
        .unwrap_err()
        .text();
    assert!(
        dispatched.contains("shares that session's inbox"),
        "{dispatched}"
    );
    let outside = h.listen(None, Some(&h.inbox())).unwrap_err().text();
    assert!(
        outside.contains("outside a Claude Code session"),
        "{outside}"
    );
    let no_inbox = h
        .listen(Some(&caller("s1", "s1")), None)
        .unwrap_err()
        .text();
    assert!(no_inbox.contains("no inbox"), "{no_inbox}");
    let unnamed = listen_command(
        &h.tool,
        &["--by".to_string(), "judge".to_string()],
        &h.scratch.dir,
        Some(&caller("s1", "s1")),
        Some(&h.inbox()),
    )
    .unwrap_err()
    .text();
    assert!(unnamed.contains("--project"), "{unnamed}");
    assert_eq!(h.kept(), None, "no refused listen was kept");
}

/// The store keeps the judge's token, and its write log leaves it out.
#[test]
fn the_judges_token_is_kept_and_not_logged() {
    let h = held("judge-secret");
    h.listen(Some(&caller("s-judge", "s-judge")), Some(&h.inbox()))
        .unwrap();
    let log = crate::lists::store::read_log(&h.store.open_existing().unwrap()).unwrap();
    assert!(!format!("{log:?}").contains(TOKEN));
}
