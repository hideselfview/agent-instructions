use super::*;
use crate::lists::fixtures::{NOON, make_store};
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::fake;
use crate::testing::{FakeInbox, Scratch, running_session};

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

#[test]
fn the_judge_listens_while_its_session_runs() {
    let h = held("judge-runs");
    let sessions = h.tool.places.claude_sessions();
    let listening = || listening(&h.store.open_existing().unwrap(), &sessions).unwrap();
    assert_eq!(listening(), None, "no session has listened");
    h.listen(Some(&caller("s-old", "s-old")), Some(&h.inbox()))
        .unwrap();
    assert_eq!(listening(), None, "the judge's session does not run");
    running_session(&h.tool.places, "s-old");
    let judge = listening().expect("the judge's session runs");
    assert_eq!(
        (judge.session.as_str(), judge.by.as_str()),
        ("s-old", "judge")
    );
    assert_eq!(judge.inbox, h.inbox());
    assert!(!format!("{judge:?}").contains(TOKEN));
    h.listen(Some(&caller("s-new", "s-new")), Some(&h.inbox()))
        .unwrap();
    assert_eq!(listening(), None, "the old judge no longer judges");
}

/// A review in demo's side worktree whose one finding nobody has settled,
/// run by <runner>.
struct Reviewed {
    demo: Demo,
    landing: i64,
    finding: i64,
    side: PathBuf,
}

fn reviewed(name: &str, runner: Option<&Caller>) -> Reviewed {
    let demo = demo(name);
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some((
                "Never do the bad thing here.",
                "a.rs",
                1,
                "a.rs",
                "The file does the bad thing.",
            )),
        )],
    );
    let reported = crate::review::review(&crate::review::Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: &side,
        base: "main",
        head: None,
        key: "branch:work",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &Stamp {
            at: NOON,
            by: "tester".to_string(),
        },
        brief: None,
        runner,
    })
    .unwrap();
    let landing = reported.landing.unwrap();
    let conn = demo.store.open_existing().unwrap();
    let finding = unsettled(&conn, landing).unwrap()[0].id;
    Reviewed {
        demo,
        landing,
        finding,
        side,
    }
}

impl Reviewed {
    fn to_settle(&self) -> ToSettle {
        ToSettle {
            project: "demo".to_string(),
            landing: self.landing,
            key: "branch:work".to_string(),
            worktree: self.side.clone(),
            findings: vec![self.finding],
        }
    }

    fn inbox(&self, name: &str) -> Messaging {
        Messaging::new(
            self.demo.scratch.dir.join(name).to_str().unwrap(),
            Some(TOKEN),
        )
        .unwrap()
    }

    fn tell(&self, caller: Option<&Caller>, own: Option<&Messaging>) -> String {
        tell(
            &self.demo.store.open_existing().unwrap(),
            &self.demo.tool.places.claude_sessions(),
            &self.to_settle(),
            caller,
            own,
        )
        .unwrap()
    }

    fn listen_as_judge(&self, inbox: &Messaging) {
        let stamp = Stamp {
            at: NOON,
            by: "judge".to_string(),
        };
        self.demo
            .store
            .write(&stamp, "judge listen", |db| {
                listen(db, inbox, &caller("s-judge", "s-judge"), &stamp)
            })
            .unwrap();
    }

    fn settle(&self, how: &str, why: &str) {
        let stamp = Stamp {
            at: NOON,
            by: "judge".to_string(),
        };
        self.demo
            .store
            .write(&stamp, "settle", |db| {
                record::settle(db, self.finding, how, why, NOON, "judge")
            })
            .unwrap();
        tell_waits(&self.demo.store.dir);
    }
}

/// The content of the message line an inbox received.
fn message(lines: &[String]) -> String {
    let line: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    line["message"]["content"].as_str().unwrap().to_string()
}

#[test]
fn a_dispatched_agents_review_records_it_as_the_branchs_agent() {
    let agent = caller("s1", "a1");
    let r = reviewed("judge-branch", Some(&agent));
    let conn = r.demo.store.open_existing().unwrap();
    let recorded: Vec<(String, String)> = conn
        .prepare("SELECT session, agent FROM review_branch_agent")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(recorded, [("s1".to_string(), "a1".to_string())]);
    let main = reviewed("judge-main", Some(&caller("s1", "s1")));
    let none: i64 = main
        .demo
        .store
        .open_existing()
        .unwrap()
        .query_row("SELECT count(*) FROM review_branch_agent", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(none, 0, "a session's main agent works on no branch");
}

#[test]
fn a_listening_judge_is_told_the_findings_to_settle() {
    let agent = caller("s1", "a1");
    let r = reviewed("judge-tell", Some(&agent));
    let judge = r.inbox("j.sock");
    r.listen_as_judge(&judge);
    running_session(&r.demo.tool.places, "s-judge");
    let received = FakeInbox::listen(Path::new(judge.socket()));
    let coordinator = r.inbox("c.sock");
    let said = r.tell(Some(&agent), Some(&coordinator));
    let lines = received.received();
    assert!(lines[0].contains(TOKEN), "the auth line carries the token");
    let told = message(&lines);
    assert!(told.starts_with("You judge project demo"), "{told}");
    assert!(
        told.contains(&format!(
            "findings {} of review landing #{}",
            r.finding, r.landing
        )),
        "{told}"
    );
    assert!(told.contains("--project demo --by judge"), "{told}");
    assert!(said.contains("told the judge of project demo"), "{said}");
    assert!(
        said.contains(&format!("builds review wait {} --project demo", r.landing)),
        "{said}"
    );
}

#[test]
fn with_no_judge_a_dispatched_agents_coordinator_is_told() {
    let agent = caller("s1", "a1");
    let r = reviewed("judge-coord", Some(&agent));
    // A judge listened, and its session no longer runs.
    r.listen_as_judge(&r.inbox("j.sock"));
    let own = r.inbox("c.sock");
    let received = FakeInbox::listen(Path::new(own.socket()));
    let said = r.tell(Some(&agent), Some(&own));
    let told = message(&received.received());
    assert!(
        told.starts_with("No judge listens for project demo, and your agent a1 ran this review"),
        "{told}"
    );
    assert!(said.contains("builds told your coordinator"), "{said}");
    let nobody = r.tell(Some(&agent), None);
    assert!(nobody.contains("builds told nobody"), "{nobody}");
}

#[test]
fn with_no_judge_a_main_agent_or_the_person_settles_and_nobody_is_told() {
    let r = reviewed("judge-self", None);
    for who in [Some(caller("s1", "s1")), None] {
        let said = r.tell(who.as_ref(), Some(&r.inbox("never.sock")));
        assert!(said.contains("settle findings"), "{said}");
        assert!(said.contains("yourself"), "{said}");
    }
    let mut empty = r.to_settle();
    empty.findings.clear();
    let said = tell(
        &r.demo.store.open_existing().unwrap(),
        &r.demo.tool.places.claude_sessions(),
        &empty,
        None,
        None,
    )
    .unwrap();
    assert_eq!(said, "", "a review with nothing to settle tells nobody");
}

#[test]
fn a_wait_ends_when_the_findings_are_settled_and_says_how() {
    let r = reviewed("judge-wait", None);
    let store = Store::named(&r.demo.tool.places, "demo").unwrap();
    let landing = r.landing;
    let waiting = std::thread::spawn(move || wait(&store, landing, Duration::from_secs(300)));
    r.settle("confirmed", "the file does the bad thing");
    let said = waiting.join().unwrap().unwrap();
    assert!(
        said.contains(&format!(
            "review landing {landing} has no finding left to settle"
        )),
        "{said}"
    );
    assert!(
        said.contains("settled confirmed by judge: the file does the bad thing"),
        "{said}"
    );
    assert!(said.contains("1 confirmed finding is open"), "{said}");
}

#[test]
fn a_wait_whose_limit_ends_first_fails_and_one_with_nothing_to_wait_for_ends_at_once() {
    let r = reviewed("judge-limit", None);
    let refused = wait(&r.demo.store, r.landing, Duration::ZERO)
        .unwrap_err()
        .text();
    assert!(
        refused.contains(&format!(
            "nobody has settled findings {} of review landing {}",
            r.finding, r.landing
        )),
        "{refused}"
    );
    assert!(refused.contains("builds review wait"), "{refused}");
    r.settle("dropped", "the bad thing is allowed here");
    let said = wait(&r.demo.store, r.landing, Duration::ZERO).unwrap();
    assert!(said.contains("No confirmed finding is open."), "{said}");
}

impl Reviewed {
    /// Runs `builds review settle <finding> <words> --project demo --by
    /// <by>` as <caller>.
    fn settle_as(&self, caller: Option<&Caller>, words: &[&str], by: &str) -> Result<()> {
        let mut args = vec![self.finding.to_string()];
        args.extend(words.iter().map(|w| w.to_string()));
        args.extend(["--project", "demo", "--by", by].map(String::from));
        crate::review::settle(&self.demo.tool, &args, caller)
    }

    fn last_settle(&self) -> Option<String> {
        record::finding_of(&self.demo.store.open_existing().unwrap(), self.finding)
            .unwrap()
            .settled
            .map(|settled| format!("{} by {}", settled.last().how, settled.last().by))
    }
}

#[test]
fn the_agent_of_the_branch_never_settles_its_findings() {
    let agent = caller("s1", "a1");
    let r = reviewed("judge-own", Some(&agent));
    for words in [
        &["confirmed", "--why", "real"][..],
        &["dropped", "--why", "wrong"][..],
    ] {
        let refused = r
            .settle_as(Some(&agent), words, "agent")
            .unwrap_err()
            .text();
        assert!(
            refused.contains(&format!("which raised finding {}", r.finding)),
            "{refused}"
        );
    }
    assert_eq!(r.last_settle(), None);
    // With no judge listening, the coordinator, its session's main agent,
    // settles it.
    r.settle_as(
        Some(&caller("s1", "s1")),
        &["confirmed", "--why", "real"],
        "coordinator",
    )
    .unwrap();
    assert_eq!(r.last_settle().as_deref(), Some("confirmed by coordinator"));
    let refused = r
        .settle_as(
            Some(&agent),
            &["allowed", "--by-person", "--why", "the person: land it"],
            "agent",
        )
        .unwrap_err()
        .text();
    assert!(refused.contains("you never settle it"), "{refused}");
    r.settle_as(Some(&agent), &["--again", "--why", "wrong"], "agent")
        .unwrap_err();
}

#[test]
fn while_a_judge_listens_only_the_judge_or_the_person_gives_a_verdict() {
    let r = reviewed("judge-only", Some(&caller("s1", "a1")));
    r.listen_as_judge(&r.inbox("j.sock"));
    running_session(&r.demo.tool.places, "s-judge");
    let coordinator = caller("s1", "s1");
    let refused = r
        .settle_as(
            Some(&coordinator),
            &["confirmed", "--why", "real"],
            "coordinator",
        )
        .unwrap_err()
        .text();
    assert!(
        refused.contains("only the judge settles a finding"),
        "{refused}"
    );
    let dispatched_judge = caller("s-judge", "a-helper");
    r.settle_as(
        Some(&dispatched_judge),
        &["confirmed", "--why", "real"],
        "helper",
    )
    .unwrap_err();
    r.settle_as(
        Some(&caller("s-judge", "s-judge")),
        &["confirmed", "--why", "real"],
        "judge",
    )
    .unwrap();
    assert_eq!(r.last_settle().as_deref(), Some("confirmed by judge"));
    r.settle_as(
        Some(&coordinator),
        &["--again", "--why", "wrong"],
        "coordinator",
    )
    .unwrap_err();
    // The coordinator records the person's words.
    r.settle_as(
        Some(&coordinator),
        &["allowed", "--by-person", "--why", "the person: land it"],
        "coordinator",
    )
    .unwrap();
}

#[test]
fn the_person_settles_whether_or_not_a_judge_listens() {
    let r = reviewed("judge-person", Some(&caller("s1", "a1")));
    r.listen_as_judge(&r.inbox("j.sock"));
    running_session(&r.demo.tool.places, "s-judge");
    r.settle_as(None, &["dropped", "--why", "wrong"], "person")
        .unwrap();
    assert_eq!(r.last_settle().as_deref(), Some("dropped by person"));
}
