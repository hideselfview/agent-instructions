use super::*;
use crate::lists::fixtures::{NOON, make_store};
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::fake;
use crate::testing::{FakeInbox, Scratch, running_session};

pub(in crate::review) const TOKEN: &str = "judge-token-7c1d";

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

pub(in crate::review) fn stamp(by: &str) -> Stamp {
    Stamp {
        at: NOON,
        by: by.to_string(),
    }
}

pub(in crate::review) fn caller(session: &str, id: &str) -> Caller {
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
pub(in crate::review) struct Reviewed {
    pub(in crate::review) demo: Demo,
    pub(in crate::review) landing: i64,
    pub(in crate::review) finding: i64,
    pub(in crate::review) side: PathBuf,
}

pub(in crate::review) fn reviewed(name: &str, runner: Option<&Caller>) -> Reviewed {
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
    let finding = waiting(&conn, landing).unwrap()[0].id;
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
            worktree: Some(self.side.clone()),
            findings: vec![self.finding],
        }
    }

    pub(in crate::review) fn inbox(&self, name: &str) -> Messaging {
        Messaging::new(
            self.demo.scratch.dir.join(name).to_str().unwrap(),
            Some(TOKEN),
        )
        .unwrap()
    }

    fn tell(&self, caller: Option<&Caller>, own: Option<&Messaging>) -> String {
        tell(
            &self.demo.store,
            &stamp("tester"),
            &self.demo.tool.places.claude_sessions(),
            &self.to_settle().notice().unwrap(),
            caller,
            own,
        )
        .unwrap()
    }

    pub(in crate::review) fn listen_as_judge(&self, inbox: &Messaging) {
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
        tell_waits(&self.demo.store.dir).unwrap();
    }
}

/// The content of the message line an inbox received.
pub(in crate::review) fn message(lines: &[String]) -> String {
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
        told.starts_with(
            "No judge listens for project demo, and your agent a1 ran a review: findings"
        ),
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
    assert!(
        empty.notice().is_none(),
        "a review with nothing to settle tells nobody"
    );
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
            "findings {} of review landing {} still wait for a settle",
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
    pub(in crate::review) fn settle_as(
        &self,
        caller: Option<&Caller>,
        words: &[&str],
        by: &str,
    ) -> Result<()> {
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

impl Reviewed {
    /// Runs `builds review <command> <finding> --why <why> --project demo
    /// --by <by>` as <caller>, for `dispute` or `keep`.
    pub(in crate::review) fn answer_as(
        &self,
        command: &str,
        caller: Option<&Caller>,
        why: &str,
        by: &str,
    ) -> Result<()> {
        let args: Vec<String> = [
            &self.finding.to_string(),
            "--why",
            why,
            "--project",
            "demo",
            "--by",
            by,
        ]
        .map(String::from)
        .to_vec();
        match command {
            "dispute" => crate::review::dispute_command(&self.demo.tool, &args, caller, None),
            "keep" => crate::review::keep_command(&self.demo.tool, &args, caller),
            other => panic!("no command {other}"),
        }
    }

    /// The dispute the store records with its answer, and what telling the
    /// settler of it says to <caller>.
    fn dispute(&self, caller: Option<&Caller>, own: Option<&Messaging>) -> String {
        let conn = self.demo.store.open_existing().unwrap();
        let finding = record::finding_of(&conn, self.finding).unwrap();
        tell(
            &self.demo.store,
            &stamp("tester"),
            &self.demo.tool.places.claude_sessions(),
            &dispute_notice(&conn, "demo", &finding).unwrap(),
            caller,
            own,
        )
        .unwrap()
    }
}

/// A review by agent a1 whose finding the judge confirmed, with the judge
/// listening on its inbox j.sock.
pub(in crate::review) fn confirmed(name: &str) -> (Reviewed, Caller) {
    let agent = caller("s1", "a1");
    let r = reviewed(name, Some(&agent));
    r.listen_as_judge(&r.inbox("j.sock"));
    running_session(&r.demo.tool.places, "s-judge");
    r.settle_as(
        Some(&caller("s-judge", "s-judge")),
        &["confirmed", "--why", "the file does the bad thing"],
        "judge",
    )
    .unwrap();
    (r, agent)
}

#[test]
fn a_dispute_tells_the_judge_and_the_wait_ends_when_the_judge_keeps_the_finding() {
    let (r, agent) = confirmed("judge-dispute");
    r.answer_as("dispute", Some(&agent), "the rule allows it here", "agent")
        .unwrap();
    let judge = r.inbox("j.sock");
    let received = FakeInbox::listen(Path::new(judge.socket()));
    let said = r.dispute(Some(&agent), None);
    let told = message(&received.received());
    assert!(
        told.contains(&format!(
            "agent thinks confirmed finding {} of review landing #{}",
            r.finding, r.landing
        )),
        "{told}"
    );
    assert!(told.contains("is wrong: the rule allows it here"), "{told}");
    assert!(
        told.contains(&format!(
            "builds review keep {} --why TEXT --project demo --by judge",
            r.finding
        )),
        "{told}"
    );
    assert!(said.contains("of your dispute of finding"), "{said}");
    assert!(said.contains("builds review wait"), "{said}");

    let store = Store::named(&r.demo.tool.places, "demo").unwrap();
    let landing = r.landing;
    let waiting = std::thread::spawn(move || wait(&store, landing, Duration::from_secs(300)));
    let refused = r
        .answer_as("keep", Some(&caller("s1", "s1")), "real", "coordinator")
        .unwrap_err()
        .text();
    assert!(
        refused.contains("only the judge settles a finding"),
        "{refused}"
    );
    r.answer_as(
        "keep",
        Some(&caller("s-judge", "s-judge")),
        "the rule names this case",
        "judge",
    )
    .unwrap();
    let said = waiting.join().unwrap().unwrap();
    assert!(
        said.contains("disputed by agent: the rule allows it here"),
        "{said}"
    );
    assert!(
        said.contains("kept by judge after the dispute: the rule names this case"),
        "{said}"
    );
    assert!(said.contains("1 confirmed finding is open"), "{said}");
}

#[test]
fn a_finding_has_one_dispute_and_a_second_disagreement_goes_to_the_coordinator() {
    let (r, agent) = confirmed("judge-dispute-once");
    let judge = caller("s-judge", "s-judge");
    let refused = r
        .answer_as("keep", Some(&judge), "real", "judge")
        .unwrap_err()
        .text();
    assert!(refused.contains("nobody disputed finding"), "{refused}");
    // Nothing reads the judge's inbox, and the agent's session has none:
    // builds records the dispute and tells nobody.
    r.answer_as("dispute", Some(&agent), "wrong", "agent")
        .unwrap();
    let waits = r
        .answer_as("dispute", Some(&agent), "wrong again", "agent")
        .unwrap_err()
        .text();
    assert!(waits.contains("waits for its answer"), "{waits}");
    r.answer_as("keep", Some(&judge), "real", "judge").unwrap();
    let second = r
        .answer_as("dispute", Some(&agent), "still wrong", "agent")
        .unwrap_err()
        .text();
    assert!(
        second.contains("a second disagreement goes to the coordinator"),
        "{second}"
    );
}

#[test]
fn settling_a_disputed_finding_again_as_dropped_answers_the_dispute() {
    let (r, agent) = confirmed("judge-dispute-drop");
    r.answer_as("dispute", Some(&agent), "the rule allows it here", "agent")
        .unwrap();
    let conn = r.demo.store.open_existing().unwrap();
    assert_eq!(waiting(&conn, r.landing).unwrap().len(), 1);
    r.settle_as(
        Some(&caller("s-judge", "s-judge")),
        &["--again", "--why", "the rule allows it here"],
        "judge",
    )
    .unwrap();
    assert!(waiting(&conn, r.landing).unwrap().is_empty());
    let said = wait(&r.demo.store, r.landing, Duration::ZERO).unwrap();
    assert!(said.contains("No confirmed finding is open."), "{said}");
    let refused = r
        .answer_as("dispute", Some(&agent), "x", "agent")
        .unwrap_err()
        .text();
    assert!(refused.contains("no open confirmed finding"), "{refused}");
}

#[test]
fn a_finding_with_no_landing_is_broken_state_that_names_the_finding() {
    let finding = Finding {
        id: 7,
        run_id: 3,
        kind: "review".to_string(),
        landing_id: None,
        group: "g".to_string(),
        rule: "r".to_string(),
        clause: "c".to_string(),
        file: "f".to_string(),
        line: 1,
        code: "x".to_string(),
        problem: "p".to_string(),
        blocking: true,
        check_failed: None,
        settled: None,
        closed_by: None,
    };
    let refused = landing_of(&finding).unwrap_err().text();
    assert!(
        refused.contains("finding 7 of run 3 belongs to no review landing"),
        "{refused}"
    );
}

#[test]
fn a_settle_that_cannot_tell_the_waits_fails_and_says_the_settle_stands() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("judge-tell-waits");
    let dir = settles_dir(&scratch.dir);
    std::fs::create_dir_all(&dir).unwrap();
    tell_waits(&scratch.dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let refused = tell_waits(&scratch.dir).unwrap_err().text();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        refused.contains("builds recorded the settle, but could not tell the waits"),
        "{refused}"
    );
    assert!(refused.contains("stop it and run it again"), "{refused}");
}
