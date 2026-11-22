use super::*;
use crate::lists::fixtures::db;
use crate::review::record::{Kind, NewCall, NewExchange, NewRun, add_landing, add_run};

fn call(exchanges: Vec<NewExchange>) -> NewCall {
    NewCall {
        group: "g".to_string(),
        model: "sonnet".to_string(),
        prompt: "the prompt".to_string(),
        rules: vec!["r".to_string()],
        usage: crate::review::call::Usage {
            tokens: crate::review::call::Tokens {
                input: Some(0),
                output: Some(0),
                cached: Some(0),
                written: Some(0),
                uncached: None,
                reasoning: None,
            },
            cost_microdollars: Some(0),
        },
        sites: None,
        failure: None,
        verdicts: Vec::new(),
        findings: Vec::new(),
        follow_up: Default::default(),
        exchanges,
    }
}

fn exchange(unit: &str, started_at: i64, ms: i64) -> NewExchange {
    NewExchange {
        model: "sonnet".to_string(),
        unit: unit.to_string(),
        pass: 0,
        system: "system".to_string(),
        message: format!("message {unit}"),
        output: "{}".to_string(),
        tokens: crate::review::call::Tokens {
            input: Some(0),
            output: Some(0),
            cached: Some(0),
            written: Some(0),
            uncached: None,
            reasoning: None,
        },
        started_at,
        ms,
    }
}

#[test]
fn each_exchange_a_run_stores_keeps_its_start_and_its_time() {
    let db = db();
    let landing = add_landing(
        &db,
        crate::review::record::Change {
            repo: "bae",
            key: "key",
        },
        1_790_000_000,
    )
    .unwrap();
    add_run(
        &db,
        &crate::review::texts::scratch(),
        &NewRun {
            landing: Some(landing),
            kind: Kind::Review { changed_lines: 3 },
            label: None,
            at: 1_790_000_000,
            base: "b",
            head: "h",
            since: None,
            rules_version: "",
        },
        &[call(vec![
            exchange("a.rs", 1_790_000_010, 4_000),
            exchange("b.rs", 1_790_000_012, 9_500),
        ])],
    )
    .unwrap();
    let times: Vec<(String, i64, i64)> = db
        .prepare(
            "SELECT e.unit, t.started_at, t.ms FROM review_exchange_time t \
             JOIN review_exchange e ON e.id = t.exchange_id ORDER BY e.id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(
        times,
        vec![
            ("a.rs".to_string(), 1_790_000_010, 4_000),
            ("b.rs".to_string(), 1_790_000_012, 9_500),
        ]
    );
}

#[test]
fn the_table_refuses_a_second_time_for_one_exchange() {
    let db = db();
    let landing = add_landing(
        &db,
        crate::review::record::Change {
            repo: "bae",
            key: "key",
        },
        1_790_000_000,
    )
    .unwrap();
    add_run(
        &db,
        &crate::review::texts::scratch(),
        &NewRun {
            landing: Some(landing),
            kind: Kind::Review { changed_lines: 3 },
            label: None,
            at: 1_790_000_000,
            base: "b",
            head: "h",
            since: None,
            rules_version: "",
        },
        &[call(vec![exchange("a.rs", 1_790_000_010, 4_000)])],
    )
    .unwrap();
    assert!(record(&db, 1, 1_790_000_020, 1).is_err());
}
