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
        grouping: "g",
    }
}

fn first_repo(db: &Connection) -> String {
    db.query_row("SELECT name FROM repo LIMIT 1", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_landing_stays_open_until_closed_and_its_reruns_are_the_groups_that_found_something() {
    let db = db();
    let repo = first_repo(&db);
    assert_eq!(open_landing(&db, "k").unwrap(), None);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    assert_eq!(open_landing(&db, "k").unwrap(), Some(landing));
    let mut failed = call("g3", None);
    failed.failure = Some("boom".to_string());
    add_run(
        &db,
        &run(Some(landing), 6, "base", "head"),
        &[
            call("g1", Some(("r", true, None))),
            call(
                "g2",
                Some(("r", true, Some("its code is not in the diff of f"))),
            ),
            failed,
            call("g4", None),
        ],
    )
    .unwrap();
    assert_eq!(groups_to_rerun(&db, landing).unwrap(), ["g3", "g1"]);
    close_landing(&db, landing, 9).unwrap();
    assert_eq!(open_landing(&db, "k").unwrap(), None);
    assert!(landing_state(&db, landing));
}

fn landing_state(db: &Connection, landing: i64) -> bool {
    super::landing(db, landing).unwrap().2
}

#[test]
fn only_a_blocking_accepted_finding_not_dropped_refuses() {
    let db = db();
    let repo = first_repo(&db);
    let landing = add_landing(&db, "k", &repo, 5).unwrap();
    let (run, ids) = add_run(
        &db,
        &run(Some(landing), 6, "b", "h"),
        &[
            call("g1", Some(("r", true, None))),
            call("g2", Some(("r", false, None))),
            call("g3", Some(("r", true, Some("no")))),
        ],
    )
    .unwrap();
    let found = findings_of_run(&db, run).unwrap();
    let refusing: Vec<i64> = found.iter().filter(|f| f.refuses()).map(|f| f.id).collect();
    assert_eq!(refusing, [ids[0][0]]);
    settle(&db, ids[0][0], "dropped", "fine", 7, "me").unwrap();
    assert!(
        !findings_of_run(&db, run)
            .unwrap()
            .iter()
            .find(|f| f.id == ids[0][0])
            .unwrap()
            .refuses()
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
