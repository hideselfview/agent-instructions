use super::*;
use crate::inbox::Messaging;
use crate::lists::fixtures::NOON;
use crate::review::fakes::fake;
use crate::review::judge::tests::{Reviewed, TOKEN, caller, confirmed, message, reviewed, stamp};
use crate::testing::{FakeInbox, running_session};

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

/// The sessions builds recorded as told of finding <finding>'s <matter>.
fn told_sessions(r: &Reviewed, finding: i64, matter: &str) -> Vec<String> {
    r.demo
        .store
        .open_existing()
        .unwrap()
        .prepare("SELECT session FROM review_judge_told WHERE finding_id = ?1 AND matter = ?2")
        .unwrap()
        .query_map(params![finding, matter], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Runs `builds judge listen --project demo --by judge` as the session
/// <session>'s main agent, on the inbox <inbox>.
fn listen(r: &Reviewed, session: &str, inbox: &Messaging) -> String {
    crate::review::judge::listen_command(
        &r.demo.tool,
        &args(&["--project", "demo", "--by", "judge"]),
        &r.demo.scratch.dir,
        Some(&caller(session, session)),
        Some(inbox),
    )
    .unwrap()
}

#[test]
fn a_finding_raised_before_the_judge_listens_reaches_it_when_it_starts_listening() {
    let r = reviewed("told-listen", Some(&caller("s1", "a1")));
    running_session(&r.demo.tool.places, "s-judge");
    let inbox = r.inbox("j.sock");
    let received = FakeInbox::listen(Path::new(inbox.socket()));
    let said = listen(&r, "s-judge", &inbox);
    let lines = received.received();
    assert!(lines[0].contains(TOKEN), "the auth line carries the token");
    let told = message(&lines);
    assert!(told.starts_with("You judge project demo"), "{told}");
    assert!(
        told.contains(&format!(
            "findings {} of review landing #{} of project demo (branch:work)",
            r.finding, r.landing
        )),
        "{told}"
    );
    assert!(said.contains("project demo's judge"), "{said}");
    assert!(
        said.contains(&format!("of findings {} of review landing", r.finding)),
        "{said}"
    );
    assert_eq!(told_sessions(&r, r.finding, "settle"), ["s-judge"]);
    // The same session that listens again is told of nothing twice: the
    // inbox is gone, so a second send would say so.
    let again = listen(&r, "s-judge", &inbox);
    assert!(!again.contains("gone"), "{again}");
    assert!(!again.contains("builds told the judge"), "{again}");
}

#[test]
fn a_judge_in_another_session_is_told_again_of_what_the_earlier_one_was_told() {
    let r = reviewed("told-replaced", Some(&caller("s1", "a1")));
    running_session(&r.demo.tool.places, "s-old");
    let old = r.inbox("old.sock");
    let received = FakeInbox::listen(Path::new(old.socket()));
    listen(&r, "s-old", &old);
    received.received();
    assert_eq!(told_sessions(&r, r.finding, "settle"), ["s-old"]);
    running_session(&r.demo.tool.places, "s-new");
    let new = r.inbox("new.sock");
    let received = FakeInbox::listen(Path::new(new.socket()));
    listen(&r, "s-new", &new);
    let told = message(&received.received());
    assert!(
        told.contains(&format!("findings {} of review landing", r.finding)),
        "{told}"
    );
    assert_eq!(told_sessions(&r, r.finding, "settle"), ["s-new"]);
}

#[test]
fn telling_the_judge_of_findings_records_each_one() {
    let agent = caller("s1", "a1");
    let r = reviewed("told-record", Some(&agent));
    let judge = r.inbox("j.sock");
    r.listen_as_judge(&judge);
    running_session(&r.demo.tool.places, "s-judge");
    let received = FakeInbox::listen(Path::new(judge.socket()));
    let notice = ToSettle {
        project: "demo".to_string(),
        landing: r.landing,
        key: "branch:work".to_string(),
        worktree: Some(r.side.clone()),
        findings: vec![r.finding],
    }
    .notice()
    .unwrap();
    let said = judge::tell(
        &r.demo.store,
        &stamp("tester"),
        &r.demo.tool.places.claude_sessions(),
        &notice,
        Some(&agent),
        None,
    )
    .unwrap();
    received.received();
    assert!(said.contains("told the judge of project demo"), "{said}");
    assert_eq!(told_sessions(&r, r.finding, "settle"), ["s-judge"]);
}

/// A second review in demo, of branch <branch> in side worktree <n>, whose
/// one finding nobody has settled: its finding.
fn another_review(r: &Reviewed, n: usize, branch: &str) -> i64 {
    let fake = fake(&r.demo.scratch.dir);
    let side = r.demo.add_side(n, "agent", branch);
    r.demo.commit(&side, "a.rs");
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
        store: &r.demo.store,
        project: "demo",
        repo: "demo",
        worktree: &side,
        base: "main",
        head: None,
        key: &format!("branch:{branch}"),
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp("tester"),
        brief: None,
        runner: None,
    })
    .unwrap();
    let conn = r.demo.store.open_existing().unwrap();
    record::findings_of_landing(&conn, reported.landing.unwrap())
        .unwrap()
        .remove(0)
        .id
}

/// Settles finding <finding> as dropped, as the judge session s-judge's main
/// agent, with the command `builds review settle`.
fn settle_as_judge(r: &Reviewed, finding: i64) {
    crate::review::settle(
        &r.demo.tool,
        &args(&[
            &finding.to_string(),
            "dropped",
            "--why",
            "the rule allows it here",
            "--project",
            "demo",
            "--by",
            "judge",
        ]),
        Some(&caller("s-judge", "s-judge")),
    )
    .unwrap();
}

#[test]
fn a_settle_sends_the_judge_a_finding_that_waits_and_reached_it_in_no_message() {
    let r = reviewed("told-settle", Some(&caller("s1", "a1")));
    let other = another_review(&r, 2, "work2");
    // The judge listens as a session that started before this review ran:
    // nothing told it of either finding.
    let judge = r.inbox("j.sock");
    r.listen_as_judge(&judge);
    running_session(&r.demo.tool.places, "s-judge");
    let received = FakeInbox::listen(Path::new(judge.socket()));
    settle_as_judge(&r, other);
    let told = message(&received.received());
    assert!(
        told.contains(&format!(
            "findings {} of review landing #{}",
            r.finding, r.landing
        )),
        "{told}"
    );
    assert!(!told.contains(&format!("findings {other} ")), "{told}");
    assert_eq!(told_sessions(&r, r.finding, "settle"), ["s-judge"]);
    assert!(told_sessions(&r, other, "settle").is_empty());
}

#[test]
fn a_dispute_that_reached_no_judge_is_sent_when_the_judge_settles_another_finding() {
    let (r, agent) = confirmed("told-dispute");
    // The judge's inbox is gone: the dispute is recorded and reaches nobody.
    r.answer_as("dispute", Some(&agent), "the rule allows it here", "agent")
        .unwrap();
    assert!(told_sessions(&r, r.finding, "answer").is_empty());
    let other = another_review(&r, 2, "work2");
    let received = FakeInbox::listen(Path::new(r.inbox("j.sock").socket()));
    settle_as_judge(&r, other);
    let told = message(&received.received());
    assert!(
        told.contains(&format!(
            "agent thinks confirmed finding {} of review landing #{}",
            r.finding, r.landing
        )),
        "{told}"
    );
    assert_eq!(told_sessions(&r, r.finding, "answer"), ["s-judge"]);
}

fn health_tool(r: &Reviewed, seconds_after_noon: i64) -> crate::tool::Tool {
    let mut tool = r.demo.scratch.tool(0);
    tool.now = NOON + seconds_after_noon;
    tool
}

#[test]
fn health_sends_the_judge_again_what_waited_longer_than_a_few_minutes() {
    let r = reviewed("told-health", Some(&caller("s1", "a1")));
    let judge = r.inbox("j.sock");
    r.listen_as_judge(&judge);
    running_session(&r.demo.tool.places, "s-judge");
    let early = crate::health_fix::tell_judges(&health_tool(&r, QUIET_SECONDS - 1)).unwrap();
    assert!(early.is_empty(), "{early:?}");
    let received = FakeInbox::listen(Path::new(judge.socket()));
    let lines = crate::health_fix::tell_judges(&health_tool(&r, QUIET_SECONDS)).unwrap();
    let told = message(&received.received());
    assert!(
        told.contains(&format!(
            "findings {} of review landing #{}",
            r.finding, r.landing
        )),
        "{told}"
    );
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("builds told the judge judge"),
        "{lines:?}"
    );
    assert_eq!(told_sessions(&r, r.finding, "settle"), ["s-judge"]);
    let quiet = crate::health_fix::tell_judges(&health_tool(&r, QUIET_SECONDS + 60)).unwrap();
    assert!(
        quiet.is_empty(),
        "told once it reaches the judge: {quiet:?}"
    );
}

#[test]
fn health_sends_the_judge_again_a_dispute_that_waited_for_its_answer() {
    let (r, agent) = confirmed("told-health-dispute");
    r.answer_as("dispute", Some(&agent), "the rule allows it here", "agent")
        .unwrap();
    let received = FakeInbox::listen(Path::new(r.inbox("j.sock").socket()));
    // The dispute began at the tool's own clock, not at NOON.
    let mut tool = r.demo.scratch.tool(0);
    tool.now = r.demo.tool.now + QUIET_SECONDS;
    let lines = crate::health_fix::tell_judges(&tool).unwrap();
    // Checked before reading the inbox: a send that never happened would
    // leave the read waiting for a connection.
    assert!(lines[0].contains("the dispute of finding"), "{lines:?}");
    let told = message(&received.received());
    assert!(told.contains("is wrong: the rule allows it here"), "{told}");
}

#[test]
fn health_forgets_a_judge_whose_session_ended_so_the_coordinator_settles() {
    let r = reviewed("told-health-ended", Some(&caller("s1", "a1")));
    r.listen_as_judge(&r.inbox("j.sock"));
    let lines = crate::health_fix::tell_judges(&health_tool(&r, 0)).unwrap();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("ended"), "{lines:?}");
    assert!(
        lines[0].contains(&format!("findings {} wait", r.finding)),
        "{lines:?}"
    );
    let kept: i64 = r
        .demo
        .store
        .open_existing()
        .unwrap()
        .query_row("SELECT count(*) FROM review_judge", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, 0);
    let again = crate::health_fix::tell_judges(&health_tool(&r, 0)).unwrap();
    assert!(again.is_empty(), "{again:?}");
}
