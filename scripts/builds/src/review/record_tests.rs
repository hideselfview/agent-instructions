use super::*;
use crate::lists::fixtures::db;

fn call(group: &str, finding: Option<(&str, bool, Option<&str>)>) -> NewCall {
    NewCall {
        group: group.to_string(),
        model: "sonnet".to_string(),
        rules: vec!["r".to_string()],
        tokens_in: 10,
        tokens_out: 2,
        tokens_cached: 4,
        tokens_written: 3,
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
        commits: &[],
        grouping: "g",
        rules_version: "",
    }
}

fn first_repo(db: &Connection) -> String {
    db.query_row("SELECT name FROM repo LIMIT 1", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_landing_stays_open_until_it_lands_and_keeps_the_commits_each_run_read() {
    let db = db();
    let repo = first_repo(&db);
    assert_eq!(open_landing(&db, "k").unwrap(), None);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    assert_eq!(open_landing(&db, "k").unwrap(), Some(landing));
    let read = [
        Commit {
            sha: "a1".to_string(),
            patch_id: "p1".to_string(),
        },
        Commit {
            sha: "b2".to_string(),
            patch_id: "p2".to_string(),
        },
    ];
    let mut with_commits = run(Some(landing), 6, "base", "head");
    with_commits.commits = &read;
    let (run_id, _) = add_run(&db, &with_commits, &[call("g1", None)]).unwrap();
    assert_eq!(commits_of_run(&db, run_id).unwrap(), read);
    assert_eq!(landings_of_repo(&db, &repo, false).unwrap()[0].id, landing);
    record_landed(&db, landing, run_id, 9).unwrap();
    assert_eq!(open_landing(&db, "k").unwrap(), None);
    assert_eq!(
        super::landing(&db, landing).unwrap().landed_run,
        Some(run_id)
    );
    assert_eq!(landings_of_repo(&db, &repo, true).unwrap()[0].id, landing);
    assert!(
        record_landed(&db, landing, run_id, 10)
            .unwrap_err()
            .text()
            .contains("not open")
    );
}

#[test]
fn only_a_confirmed_finding_of_an_open_landing_takes_a_fix_and_a_later_fix_replaces_it() {
    let db = db();
    let repo = first_repo(&db);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    let (run_id, ids) = add_run(
        &db,
        &run(Some(landing), 6, "b", "h"),
        &[call("g1", Some(("r", true, None)))],
    )
    .unwrap();
    let finding = ids[0][0];
    let fix = |sha: &str| Commit {
        sha: sha.to_string(),
        patch_id: format!("patch-{sha}"),
    };
    assert!(
        record_fix(&db, finding, &fix("c3"), 7, "me")
            .unwrap_err()
            .text()
            .contains("not settled as confirmed")
    );
    settle(&db, finding, "confirmed", "real", 7, "me").unwrap();
    record_fix(&db, finding, &fix("c3"), 8, "me").unwrap();
    record_fix(&db, finding, &fix("d4"), 9, "me").unwrap();
    assert_eq!(finding_of(&db, finding).unwrap().fix, Some(fix("d4")));
    record_landed(&db, landing, run_id, 10).unwrap();
    assert!(
        record_fix(&db, finding, &fix("e5"), 11, "me")
            .unwrap_err()
            .text()
            .contains("landed already")
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
            .contains("dropped by builds")
    );
    settle(&db, ids[1][0], "confirmed", "x", 7, "me").unwrap();
    assert!(settle(&db, ids[1][0], "dropped", "y", 8, "me").is_err());
    assert!(settle(&db, 999, "dropped", "y", 8, "me").is_err());
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
    assert_eq!(texts, 3, "the shared system prompt is kept once");
    let kept: String = db
        .query_row(
            "SELECT t.text FROM review_exchange e JOIN review_text t ON t.hash = e.message_hash WHERE e.unit = 'other'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, "another message");
}
