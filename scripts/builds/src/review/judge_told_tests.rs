use std::path::Path;

use super::*;
use crate::inbox::Messaging;
use crate::review::exchange_time;
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
        .prepare("SELECT told_session FROM review_judge_told WHERE finding_id = ?1 AND matter = ?2 AND told_session IS NOT NULL")
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
    end_writers(&r.demo.store);
    running_session(&r.demo.tool.places, "s-judge");
    let inbox = r.inbox("j.sock");
    let received = FakeInbox::listen(Path::new(inbox.stored().1));
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
    assert!(said.contains("judges project demo"), "{said}");
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
    end_writers(&r.demo.store);
    running_session(&r.demo.tool.places, "s-old");
    let old = r.inbox("old.sock");
    let received = FakeInbox::listen(Path::new(old.stored().1));
    listen(&r, "s-old", &old);
    received.received();
    assert_eq!(told_sessions(&r, r.finding, "settle"), ["s-old"]);
    running_session(&r.demo.tool.places, "s-new");
    let new = r.inbox("new.sock");
    let received = FakeInbox::listen(Path::new(new.stored().1));
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
    let received = FakeInbox::listen(Path::new(judge.stored().1));
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
        &r.demo.tool.sessions(),
        &crate::testing::running_claude(&r.demo.tool.places),
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
    end_writers(&r.demo.store);
    // The judge listens as a session that started before this review ran:
    // nothing told it of either finding.
    let judge = r.inbox("j.sock");
    r.listen_as_judge(&judge);
    running_session(&r.demo.tool.places, "s-judge");
    let received = FakeInbox::listen(Path::new(judge.stored().1));
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
    end_writers(&r.demo.store);
    let received = FakeInbox::listen(Path::new(r.inbox("j.sock").stored().1));
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

/// Runs `builds health`'s fix for the judges, in the scratch folder of <r>,
/// and returns its lines about findings: the tests here give the judge no
/// acknowledgement, which judge_ack_tests.rs covers.
fn health(r: &Reviewed) -> Vec<String> {
    let mut lines = crate::health_fix::tell_judges(
        &r.demo.scratch.tool(0),
        std::slice::from_ref(&r.demo.store),
        &crate::claude_sessions::running_now(&r.demo.scratch.tool(0).places.claude_sessions()),
    );
    lines.retain(|line| !line.contains(crate::review::judge_ack::ACKNOWLEDGE));
    lines
}

#[test]
fn health_sends_nothing_while_the_process_that_made_the_finding_wait_runs() {
    let r = reviewed("told-alive", Some(&caller("s1", "a1")));
    r.listen_as_judge(&r.inbox("j.sock"));
    running_session(&r.demo.tool.places, "s-judge");
    // This test process wrote the rows and runs: it is telling the judge.
    // Nothing listens on the judge's inbox, so a send would say it is gone.
    assert!(health(&r).is_empty());
    let said = listen(&r, "s-judge", &r.inbox("j.sock"));
    assert!(!said.contains("gone"), "{said}");
    assert!(!said.contains("builds told the judge"), "{said}");
    assert!(told_sessions(&r, r.finding, "settle").is_empty());
}

#[test]
fn a_process_with_the_pid_of_the_writer_and_another_start_time_is_another_process() {
    let r = reviewed("told-reused-pid", Some(&caller("s1", "a1")));
    r.listen_as_judge(&r.inbox("j.sock"));
    running_session(&r.demo.tool.places, "s-judge");
    let me = std::process::id();
    reuse_writers_pids(&r.demo.store);
    let received = FakeInbox::listen(Path::new(r.inbox("j.sock").stored().1));
    let lines = health(&r);
    assert_eq!(lines.len(), 1, "pid {me} is another process now: {lines:?}");
    received.received();
}

#[test]
fn health_sends_the_judge_again_what_no_running_process_is_telling_it_of() {
    let r = reviewed("told-health", Some(&caller("s1", "a1")));
    end_writers(&r.demo.store);
    let judge = r.inbox("j.sock");
    r.listen_as_judge(&judge);
    running_session(&r.demo.tool.places, "s-judge");
    let received = FakeInbox::listen(Path::new(judge.stored().1));
    let lines = health(&r);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("builds told the judge judge"),
        "{lines:?}"
    );
    let told = message(&received.received());
    assert!(
        told.contains(&format!(
            "findings {} of review landing #{}",
            r.finding, r.landing
        )),
        "{told}"
    );
    assert_eq!(told_sessions(&r, r.finding, "settle"), ["s-judge"]);
}

#[test]
fn health_sends_nothing_for_a_finding_it_already_told_the_judges_session_of() {
    let r = reviewed("told-told", Some(&caller("s1", "a1")));
    end_writers(&r.demo.store);
    let judge = r.inbox("j.sock");
    r.listen_as_judge(&judge);
    running_session(&r.demo.tool.places, "s-judge");
    let received = FakeInbox::listen(Path::new(judge.stored().1));
    assert_eq!(health(&r).len(), 1);
    received.received();
    // The inbox is gone now, so a second send would say so.
    assert!(health(&r).is_empty());
}

#[test]
fn health_sends_the_judge_again_a_dispute_whose_writer_ended() {
    let (r, agent) = confirmed("told-health-dispute");
    r.answer_as("dispute", Some(&agent), "the rule allows it here", "agent")
        .unwrap();
    end_writers(&r.demo.store);
    let received = FakeInbox::listen(Path::new(r.inbox("j.sock").stored().1));
    let lines = health(&r);
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
    let lines = health(&r);
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
    assert!(health(&r).is_empty());
}

fn health_report(r: &Reviewed) -> crate::health::Health {
    let tool = &r.demo.tool;
    let health = crate::health::health(tool).unwrap();
    let scanned = crate::health::Scanned::judge(
        &tool.places,
        Vec::new(),
        &crate::job_claim::Holders::read().unwrap(),
        tool.now,
    );
    crate::health::finish_health(
        tool,
        &crate::testing::no_ssh(),
        health,
        &mut crate::lists::store::load_all(&tool.places),
        &scanned,
        &Ok(vec![r.demo.store.clone()]),
        &crate::claude_sessions::running_now(&tool.places.claude_sessions()),
    )
    .unwrap()
    .0
}

#[test]
fn health_lists_a_project_whose_findings_wait_for_a_missing_judge() {
    let mut r = reviewed("health-missing-judge", Some(&caller("s1", "a1")));
    let oldest = another_review(&r, 2, "other");
    let recorded_at = crate::lists::fixtures::NOON + 60;
    r.demo
        .store
        .write(&stamp("tester"), "set reviewer timing", |db| {
            exchange_time::set_call_finish_for_finding(db, r.finding, recorded_at + 60)?;
            exchange_time::set_call_finish_for_finding(db, oldest, recorded_at)
        })
        .unwrap();
    r.demo.tool.now = recorded_at + 12 * 60;
    let lines = crate::health::lines(&health_report(&r));
    let expected = "    No judge listens for project demo. The oldest finding has waited 12m for a settle. Start a judge session and run `builds judge listen --project demo --by judge`";
    assert_eq!(
        lines
            .iter()
            .skip_while(|line| *line != "needs action:")
            .filter(|line| line.contains("No judge listens for project demo"))
            .collect::<Vec<_>>(),
        [expected],
        "{lines:#?}"
    );
}

#[test]
fn health_flags_a_missing_judge_only_while_a_finding_needs_a_settle() {
    let r = reviewed("health-judge-availability", Some(&caller("s1", "a1")));
    let needs_judge = || {
        health_report(&r)
            .findings
            .iter()
            .any(|finding| finding.kind == crate::health::FindingKind::Judge)
    };
    r.listen_as_judge(&r.inbox("j.sock"));
    assert!(needs_judge(), "the judge's session has ended");

    running_session(&r.demo.tool.places, "s-judge");
    assert!(!needs_judge(), "the judge listens");

    settle_as_judge(&r, r.finding);
    running_session(&r.demo.tool.places, "other-session");
    assert!(!needs_judge(), "the finding has a settle");
}

#[test]
fn health_reports_when_a_findings_wait_time_is_missing() {
    let r = reviewed("health-judge-wait-time", Some(&caller("s1", "a1")));
    r.demo
        .store
        .write(
            &stamp("tester"),
            "remove reviewer timing",
            exchange_time::clear,
        )
        .unwrap();
    let health = health_report(&r);
    let judge = health
        .findings
        .iter()
        .find(|finding| finding.kind == crate::health::FindingKind::Judge)
        .unwrap();
    assert!(
        judge.advice.text.contains("no recorded finish time"),
        "{}",
        judge.advice.text
    );
}
