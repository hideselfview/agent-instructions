use super::*;
use crate::lists::fixtures::db;

fn call(group: &str, finding: Option<(&str, bool, Option<&str>)>) -> NewCall {
    NewCall {
        group: group.to_string(),
        model: "sonnet".to_string(),
        prompt: format!("the prompt of {group}"),
        rules: vec!["r".to_string()],
        usage: crate::review::call::Usage {
            tokens: crate::review::call::Tokens {
                input: Some(10),
                output: Some(2),
                cached: Some(4),
                written: Some(3),
                uncached: None,
                reasoning: None,
            },
            cost_microdollars: Some(500_000),
        },
        sites: None,
        failure: None,
        exchanges: vec![NewExchange {
            model: "sonnet".to_string(),
            unit: "f".to_string(),
            pass: 0,
            system: "system".to_string(),
            message: "message".to_string(),
            output: "{}".to_string(),
            tokens: crate::review::call::Tokens {
                input: Some(10),
                output: Some(2),
                cached: Some(4),
                written: Some(3),
                uncached: None,
                reasoning: None,
            },
            started_at: 1_790_000_000,
            ms: 2_000,
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
        follow_up: Default::default(),
    }
}

fn run<'a>(landing: Option<i64>, at: i64, base: &'a str, head: &'a str) -> NewRun<'a> {
    NewRun {
        landing,
        kind: Kind::Review { changed_lines: 0 },
        label: None,
        at,
        base,
        head,
        since: None,
        rules_version: "",
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
    let k = Change {
        repo: &repo,
        key: "k",
    };
    assert_eq!(open_landing(&db, k).unwrap(), None);
    let landing = add_landing(&db, k, 5).unwrap();
    assert_eq!(open_landing(&db, k).unwrap(), Some(landing));
    let (run_id, _) = add_run(
        &db,
        &crate::review::texts::scratch(),
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
    assert_eq!(open_landing(&db, k).unwrap(), None);
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
    let k = Change {
        repo: &repo,
        key: "k",
    };
    let landing = add_landing(&db, k, 5).unwrap();
    let (_, ids) = add_run(
        &db,
        &crate::review::texts::scratch(),
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
    let k = Change {
        repo: &repo,
        key: "k",
    };
    let landing = add_landing(&db, k, 5).unwrap();
    let (_, ids) = add_run(
        &db,
        &crate::review::texts::scratch(),
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
    let settle_of = |how: &str, why: &str, by: &str, at: i64| Settle {
        how: how.to_string(),
        why: why.to_string(),
        by: by.to_string(),
        at,
    };
    assert_eq!(
        found.settled,
        Some(Settled::Again {
            first: settle_of("confirmed", "the code does it", "first", 7),
            second: settle_of("dropped", "another crate calls it", "second", 8),
        })
    );
    assert!(found.settled_as("dropped") && !found.settled_as("confirmed"));
    assert!(
        settle_again(&db, again, "once more", 9, "third")
            .unwrap_err()
            .text()
            .contains("again already")
    );
}

#[test]
fn a_later_run_closes_an_open_confirmed_finding_once_and_a_closed_one_stays_confirmed() {
    let db = db();
    let repo = first_repo(&db);
    let k = Change {
        repo: &repo,
        key: "k",
    };
    let landing = add_landing(&db, k, 5).unwrap();
    let (_, ids) = add_run(
        &db,
        &crate::review::texts::scratch(),
        &run(Some(landing), 6, "b", "h"),
        &[call("g1", Some(("r", true, None)))],
    )
    .unwrap();
    let finding = ids[0][0];
    let mut closing = call("g1", None);
    closing
        .follow_up
        .insert(finding, super::super::call::FollowUp::Fixed);
    assert!(
        add_run(
            &db,
            &crate::review::texts::scratch(),
            &run(Some(landing), 7, "b", "h2"),
            std::slice::from_ref(&closing)
        )
        .unwrap_err()
        .text()
        .contains("not an open confirmed finding")
    );
    settle(&db, finding, "confirmed", "real", 7, "me").unwrap();
    assert!(finding_of(&db, finding).unwrap().is_open());
    let other = add_landing(
        &db,
        Change {
            repo: &repo,
            key: "k2",
        },
        5,
    )
    .unwrap();
    assert!(
        add_run(
            &db,
            &crate::review::texts::scratch(),
            &run(Some(other), 8, "b", "h2"),
            std::slice::from_ref(&closing)
        )
        .unwrap_err()
        .text()
        .contains("not a finding of the landing")
    );
    let (closer, _) = add_run(
        &db,
        &crate::review::texts::scratch(),
        &run(Some(landing), 8, "b", "h2"),
        &[closing],
    )
    .unwrap();
    let found = finding_of(&db, finding).unwrap();
    assert_eq!(found.closed_by, Some(closer));
    assert!(!found.is_open() && found.settled_as("confirmed"));
    assert!(
        settle_again(&db, finding, "wrong", 9, "me")
            .unwrap_err()
            .text()
            .contains("closed finding")
    );
    assert!(
        allow(&db, finding, "the person: land it", 9, "me")
            .unwrap_err()
            .text()
            .contains("closed finding")
    );
}

#[test]
fn the_person_allows_an_open_confirmed_finding_which_stays_confirmed_and_is_no_longer_open() {
    let db = db();
    let repo = first_repo(&db);
    let k = Change {
        repo: &repo,
        key: "k",
    };
    let landing = add_landing(&db, k, 5).unwrap();
    let (_, ids) = add_run(
        &db,
        &crate::review::texts::scratch(),
        &run(Some(landing), 6, "b", "h"),
        &[
            call("g1", Some(("r", true, None))),
            call("g2", Some(("r", true, None))),
            call("g3", Some(("r", true, Some("gone")))),
        ],
    )
    .unwrap();
    let (allowed, dropped, checked) = (ids[0][0], ids[1][0], ids[2][0]);
    assert!(
        allow(&db, allowed, "the person: land it", 7, "me")
            .unwrap_err()
            .text()
            .contains("not settled")
    );
    assert!(
        allow(&db, checked, "the person: land it", 7, "me")
            .unwrap_err()
            .text()
            .contains("builds' own check dropped finding")
    );
    settle(&db, dropped, "dropped", "wrong", 7, "me").unwrap();
    assert!(
        allow(&db, dropped, "the person: land it", 8, "me")
            .unwrap_err()
            .text()
            .contains("as dropped already")
    );
    settle(&db, allowed, "confirmed", "real", 7, "me").unwrap();
    assert!(allow(&db, allowed, "  ", 8, "me").is_err());
    allow(&db, allowed, "the person: land it", 8, "me").unwrap();
    let found = finding_of(&db, allowed).unwrap();
    assert_eq!(
        found.settled,
        Some(Settled::Allowed {
            confirmed: Settle {
                how: "confirmed".to_string(),
                why: "real".to_string(),
                by: "me".to_string(),
                at: 7,
            },
            allowed: Allowed {
                why: "the person: land it".to_string(),
                by: "me".to_string(),
            },
        })
    );
    assert!(!found.is_open() && found.settled_as("confirmed"));
    assert!(
        allow(&db, allowed, "again", 9, "me")
            .unwrap_err()
            .text()
            .contains("to land already")
    );
    assert!(
        settle_again(&db, allowed, "wrong", 9, "me")
            .unwrap_err()
            .text()
            .contains("stays confirmed")
    );
}

#[test]
fn a_review_run_needs_a_landing() {
    let db = db();
    assert!(
        add_run(
            &db,
            &crate::review::texts::scratch(),
            &run(None, 1, "b", "h"),
            &[]
        )
        .is_err()
    );
}

#[test]
fn every_exchange_of_a_call_is_kept_with_the_hashes_of_its_texts_and_the_run_keeps_the_texts_once_each()
 {
    let db = db();
    let texts = crate::review::texts::scratch();
    let repo = first_repo(&db);
    let k = Change {
        repo: &repo,
        key: "k",
    };
    let landing = add_landing(&db, k, 5).unwrap();
    let mut one = call("g", None);
    let mut second = one.exchanges[0].clone();
    second.unit = "other".to_string();
    second.message = "another message".to_string();
    second.output = "no answer: boom".to_string();
    one.exchanges.push(second);
    let (run, _) = add_run(&db, &texts, &run(Some(landing), 6, "b", "h"), &[one]).unwrap();
    let exchanges: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM review_exchange e JOIN review_call c ON c.id = e.call_id WHERE c.run_id = ?1",
            [run],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exchanges, 2);
    let hash: String = db
        .query_row(
            "SELECT message_hash FROM review_exchange WHERE unit = 'other'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(texts.read(run, &hash).unwrap(), "another message");
    assert_eq!(
        messages_of_call(&db, &texts, run, "g").unwrap(),
        ["message", "another message"]
    );
}

#[test]
fn a_run_whose_texts_the_cap_deleted_keeps_its_records_and_refuses_to_read_its_messages() {
    let db = db();
    let texts = crate::review::texts::scratch();
    let repo = first_repo(&db);
    let k = Change {
        repo: &repo,
        key: "k",
    };
    let landing = add_landing(&db, k, 5).unwrap();
    let (run, _) = add_run(
        &db,
        &texts,
        &run(Some(landing), 6, "b", "h"),
        &[call("g", Some(("r", true, None)))],
    )
    .unwrap();
    // A run of another id that holds no texts stands for the deleted ones.
    let why = messages_of_call(&db, &crate::review::texts::scratch(), run, "g")
        .unwrap_err()
        .text();
    assert!(why.contains("keeps no text"), "{why}");
    assert_eq!(findings_of_run(&db, run).unwrap().len(), 1);
}

#[test]
fn branches_of_one_name_in_two_repos_are_two_changes_and_a_change_opens_one_landing() {
    let db = db();
    let bae = Change {
        repo: "bae",
        key: "branch:work",
    };
    let coven = Change {
        repo: "coven",
        key: "branch:work",
    };
    let in_bae = add_landing(&db, bae, 5).unwrap();
    assert_eq!(open_landing(&db, coven).unwrap(), None);
    let in_coven = add_landing(&db, coven, 6).unwrap();
    assert_ne!(in_bae, in_coven);
    assert_eq!(open_landing(&db, bae).unwrap(), Some(in_bae));
    assert_eq!(open_landing(&db, coven).unwrap(), Some(in_coven));
    assert!(add_landing(&db, bae, 7).is_err());
}

/// Writes, in the schema from before the review texts left the store, a replay
/// run 4 with call 9 and exchange 6, whose prompt, system and message texts
/// the store holds as rows of review_text under the hashes <prompt>,
/// <system> and <message>: the state migrate.rs's move of the review texts
/// to the build volume starts from. The current schema holds no such rows,
/// so no write of this file's can build them.
pub fn plant_run_with_texts_in_the_store(
    db: &Connection,
    (prompt, system, message): (&str, &str, &str),
) -> rusqlite::Result<()> {
    db.execute_batch(&format!(
        "INSERT INTO review_run (id, kind, at, base, head, rules_version) VALUES (4, 'replay', 5, 'b', 'h', '');
INSERT INTO review_text (hash, text) VALUES ('{prompt}', 'the prompt'), ('{system}', 'the system'), ('{message}', 'the message');
INSERT INTO review_call (id, run_id, group_name, model, prompt, rules, tokens_in, tokens_out, tokens_cached, tokens_written) VALUES (9, 4, 'g', 'm', '{prompt}', '[]', 1, 1, 0, 0);
INSERT INTO review_exchange (id, call_id, model, unit, pass, system_hash, message_hash, output, tokens_in, tokens_out, tokens_cached, tokens_written) VALUES (6, 9, 'm', 'f', 0, '{system}', '{message}', 'the answer', 1, 1, 0, 0);"
    ))
}

#[test]
fn a_review_records_each_reported_counter_and_distinguishes_zero_from_unknown() {
    use crate::review::call::{Tokens, Usage};
    let db = db();
    let repo = first_repo(&db);
    let landing = add_landing(
        &db,
        Change {
            repo: &repo,
            key: "k",
        },
        5,
    )
    .unwrap();
    let mut failed = call("failed", None);
    let partial = Tokens {
        input: Some(18),
        cached: Some(0),
        ..Tokens::default()
    };
    failed.usage = Usage {
        tokens: partial,
        cost_microdollars: None,
    };
    failed.exchanges[0].tokens = partial;
    failed.failure = Some("the provider did not report usage".to_string());
    let mut measured = call("measured", None);
    measured.usage = Usage::ZERO;
    measured.exchanges[0].tokens = Tokens::ZERO;
    add_run(
        &db,
        &crate::review::texts::scratch(),
        &run(Some(landing), 6, "base", "head"),
        &[failed.clone(), measured],
    )
    .unwrap();
    let calls = calls_of_landing(&db, landing).unwrap();
    assert_eq!(
        calls.iter().map(|call| call.usage).collect::<Vec<_>>(),
        [failed.usage, Usage::ZERO]
    );
    assert!(crate::review::render::totals(&calls, &[]).contains(
        "18 tokens in (unknown uncached, 0 read from the cache, unknown written to it), unknown out"
    ));
}

#[test]
fn migrating_failed_review_exchanges_preserves_measured_zero_and_removes_unknown_zero() {
    use crate::review::call::{Tokens, Usage};
    let db = db();
    let scratch = crate::testing::Scratch::new("migrate-review-usage");
    let places = scratch.places();
    let texts = crate::review::texts::Texts::of(&places, "demo");
    let repo = first_repo(&db);
    let landing = add_landing(
        &db,
        Change {
            repo: &repo,
            key: "k",
        },
        5,
    )
    .unwrap();
    let old = |group, failure: &str, output: &str| {
        let mut call = call(group, None);
        call.usage = Usage::ZERO;
        call.failure = Some(failure.to_string());
        call.exchanges[0].output = output.to_string();
        call.exchanges[0].tokens = Tokens::ZERO;
        call
    };
    let legacy = old(
        "failed",
        "./claude exited with exit status: 1: limit",
        "no answer: ./claude exited with exit status: 1: limit",
    );
    let measured = old(
        "measured",
        "the reviewer reported an error: subprocess exited with exit status: 1",
        "no answer: the reviewer reported an error: subprocess exited with exit status: 1",
    );
    let mut ambiguous = old(
        "ambiguous",
        "two different failures",
        "no answer: the reviewer printed no JSON",
    );
    let mut reported = measured.exchanges[0].clone();
    reported.output = "{}".to_string();
    reported.tokens = Tokens {
        input: Some(9),
        ..Tokens::ZERO
    };
    ambiguous.usage.tokens = reported.tokens;
    ambiguous.exchanges.push(reported);
    add_run(
        &db,
        &texts,
        &run(Some(landing), 6, "base", "head"),
        &[legacy, measured.clone(), ambiguous.clone()],
    )
    .unwrap();
    let run_id = db
        .query_row("SELECT MAX(id) FROM review_run", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let answers = crate::review::texts::root(&places)
        .join("demo")
        .join(run_id.to_string());
    for call in [measured, ambiguous] {
        let hash = crate::review::texts::hash(&call.exchanges[0].output);
        std::fs::remove_file(answers.join(hash)).unwrap();
    }
    db.pragma_update(None, "foreign_keys", false).unwrap();
    allow_unreported_tokens(&db, &texts).unwrap();
    let calls = calls_of_landing(&db, landing).unwrap();
    assert_eq!(calls[0].usage.tokens, Tokens::default());
    assert_eq!(
        calls[1].usage.tokens,
        Tokens {
            reasoning: None,
            ..Tokens::ZERO
        }
    );
    assert_eq!(calls[2].usage.tokens, Tokens::default());
    let results = db
        .prepare("SELECT tokens_in FROM review_exchange ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, Option<i64>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(results, [None, Some(0), None, Some(9)]);
}
