use super::*;
use crate::lists::fixtures::db;

fn call(group: &str, finding: Option<(&str, bool, Option<&str>)>) -> NewCall {
    NewCall {
        group: group.to_string(),
        model: "sonnet".to_string(),
        prompt: format!("the prompt of {group}"),
        rules: vec!["r".to_string()],
        tokens_in: 10,
        tokens_out: 2,
        tokens_cached: 4,
        tokens_written: 3,
        cost_microdollars: 500_000,
        sites: None,
        failure: None,
        exchanges: vec![NewExchange {
            model: "sonnet".to_string(),
            unit: "f".to_string(),
            pass: 0,
            system: "system".to_string(),
            message: "message".to_string(),
            output: "{}".to_string(),
            tokens_in: 10,
            tokens_out: 2,
            tokens_cached: 4,
            tokens_written: 3,
        }],
        verdicts: vec![(
            "r".to_string(),
            if finding.is_some() { "finding" } else { "none" }.to_string(),
        )],
        findings: finding
            .map(|(rule, blocking, check_failed)| NewFinding {
                rule: rule.to_string(),
                clause: "c".to_string(),
                file: "f".to_string(),
                line: 1,
                code: "x".to_string(),
                problem: "p".to_string(),
                blocking,
                check_failed: check_failed.map(str::to_string),
            })
            .into_iter()
            .collect(),
        closes: Vec::new(),
    }
}

fn run<'a>(landing: Option<i64>, at: i64, base: &'a str, head: &'a str) -> NewRun<'a> {
    NewRun {
        landing,
        kind: Kind::Review,
        label: None,
        at,
        base,
        head,
        since: None,
        grouping: "g",
        rules_version: "",
        changed_lines: None,
    }
}

fn first_repo(db: &Connection) -> String {
    db.query_row("SELECT name FROM repo LIMIT 1", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_landing_stays_open_until_it_lands_and_names_its_last_review() {
    let db = db();
    let repo = first_repo(&db);
    assert_eq!(open_landing(&db, "k").unwrap(), None);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    assert_eq!(open_landing(&db, "k").unwrap(), Some(landing));
    let (run_id, _) = add_run(
        &db,
        &run(Some(landing), 6, "base", "head"),
        &[call("g1", None)],
    )
    .unwrap();
    record_reviewed(&db, landing, run_id).unwrap();
    assert_eq!(
        super::landing(&db, landing).unwrap().reviewed_run,
        Some(run_id)
    );
    record_landed(&db, landing, run_id, 9).unwrap();
    assert_eq!(open_landing(&db, "k").unwrap(), None);
    assert_eq!(
        super::landing(&db, landing).unwrap().landed_run,
        Some(run_id)
    );
    assert!(
        record_landed(&db, landing, run_id, 10)
            .unwrap_err()
            .text()
            .contains("not open")
    );
}

#[test]
fn a_finding_builds_dropped_cannot_be_settled_and_a_settled_one_cannot_be_settled_again() {
    let db = db();
    let repo = first_repo(&db);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    let (_, ids) = add_run(
        &db,
        &run(Some(landing), 6, "b", "h"),
        &[
            call("g1", Some(("r", true, Some("gone")))),
            call("g2", Some(("r", true, None))),
        ],
    )
    .unwrap();
    assert!(
        settle(&db, ids[0][0], "dropped", "x", 7, "me")
            .unwrap_err()
            .text()
            .contains("builds' own check dropped finding")
    );
    settle(&db, ids[1][0], "confirmed", "x", 7, "me").unwrap();
    assert!(settle(&db, ids[1][0], "dropped", "y", 8, "me").is_err());
    assert!(settle(&db, 999, "dropped", "y", 8, "me").is_err());
}

#[test]
fn settling_again_drops_a_confirmed_finding_and_keeps_both_settles() {
    let db = db();
    let repo = first_repo(&db);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    let (_, ids) = add_run(
        &db,
        &run(Some(landing), 6, "b", "h"),
        &[
            call("g1", Some(("r", true, None))),
            call("g2", Some(("r", true, None))),
        ],
    )
    .unwrap();
    let (again, unsettled) = (ids[0][0], ids[1][0]);
    assert!(
        settle_again(&db, unsettled, "wrong", 7, "me")
            .unwrap_err()
            .text()
            .contains("not settled")
    );
    settle(&db, again, "confirmed", "the code does it", 7, "first").unwrap();
    assert!(
        settle(&db, again, "dropped", "wrong", 8, "second")
            .unwrap_err()
            .text()
            .contains("--again"),
        "a second settle without --again fails with an error that names --again"
    );
    settle_again(&db, again, "another crate calls it", 8, "second").unwrap();
    let found = finding_of(&db, again).unwrap();
    let settle_of = |how: &str, why: &str, by: &str| Settle {
        how: how.to_string(),
        why: why.to_string(),
        by: by.to_string(),
    };
    assert_eq!(
        found.settled,
        Some(Settled::Again {
            first: settle_of("confirmed", "the code does it", "first"),
            second: settle_of("dropped", "another crate calls it", "second"),
        })
    );
    assert!(found.settled_as("dropped") && !found.settled_as("confirmed"));
    assert!(
        settle_again(&db, again, "once more", 9, "third")
            .unwrap_err()
            .text()
            .contains("settled again already")
    );
}

#[test]
fn a_later_run_closes_an_open_confirmed_finding_once_and_a_closed_one_stays_confirmed() {
    let db = db();
    let repo = first_repo(&db);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    let (_, ids) = add_run(
        &db,
        &run(Some(landing), 6, "b", "h"),
        &[call("g1", Some(("r", true, None)))],
    )
    .unwrap();
    let finding = ids[0][0];
    let mut closing = call("g1", None);
    closing.closes = vec![finding];
    assert!(
        add_run(
            &db,
            &run(Some(landing), 7, "b", "h2"),
            std::slice::from_ref(&closing)
        )
        .unwrap_err()
        .text()
        .contains("not an open confirmed finding")
    );
    settle(&db, finding, "confirmed", "real", 7, "me").unwrap();
    assert!(finding_of(&db, finding).unwrap().is_open());
    let other = add_landing(&db, "k2", &repo, 5).unwrap();
    assert!(
        add_run(
            &db,
            &run(Some(other), 8, "b", "h2"),
            std::slice::from_ref(&closing)
        )
        .unwrap_err()
        .text()
        .contains("not a finding of the landing")
    );
    let (closer, _) = add_run(&db, &run(Some(landing), 8, "b", "h2"), &[closing]).unwrap();
    let found = finding_of(&db, finding).unwrap();
    assert_eq!(found.closed_by, Some(closer));
    assert!(!found.is_open() && found.settled_as("confirmed"));
    assert!(
        settle_again(&db, finding, "wrong", 9, "me")
            .unwrap_err()
            .text()
            .contains("builds refuses to settle a closed finding again")
    );
}

#[test]
fn a_review_run_needs_a_landing() {
    let db = db();
    assert!(add_run(&db, &run(None, 1, "b", "h"), &[]).is_err());
}

#[test]
fn every_exchange_of_a_call_is_kept_with_its_texts_once_each() {
    let db = db();
    let repo = first_repo(&db);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    let mut one = call("g", None);
    let mut second = one.exchanges[0].clone();
    second.unit = "other".to_string();
    second.message = "another message".to_string();
    second.output = "no answer: boom".to_string();
    one.exchanges.push(second);
    let (run, _) = add_run(&db, &run(Some(landing), 6, "b", "h"), &[one]).unwrap();
    let exchanges: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM review_exchange e JOIN review_call c ON c.id = e.call_id WHERE c.run_id = ?1",
            [run],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exchanges, 2);
    let texts: i64 = db
        .query_row("SELECT COUNT(*) FROM review_text", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        texts, 4,
        "the shared system prompt is kept once, beside the two messages and the call's prompt"
    );
    let kept: String = db
        .query_row(
            "SELECT t.text FROM review_exchange e JOIN review_text t ON t.hash = e.message_hash WHERE e.unit = 'other'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, "another message");
}
