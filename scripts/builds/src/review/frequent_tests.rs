use super::*;
use crate::lists::fixtures::db;
use crate::review::record::{Kind, NewCall, NewFinding, NewRun, add_landing, add_run, settle};
use crate::review::rules::Rule;

const RULE: &str = "---\npaths: ['**/*']\n---\n\n## Write plainly\n\n**Active.** Name the actor and the action.\n\n**Literal.** Use the words for what happens.\n- Name what the code does.\n  - Name the type it returns.\n";

fn rules() -> Vec<Rule> {
    vec![Rule::parse("plain", RULE)]
}

fn call(clauses: &[&str]) -> NewCall {
    NewCall {
        group: "g".to_string(),
        model: "opus".to_string(),
        prompt: "prompt".to_string(),
        rules: vec!["plain".to_string()],
        tokens_in: 0,
        tokens_out: 0,
        tokens_cached: 0,
        tokens_written: 0,
        cost_microdollars: 0,
        sites: None,
        failure: None,
        verdicts: Vec::new(),
        findings: clauses
            .iter()
            .map(|clause| NewFinding {
                rule: "plain".to_string(),
                clause: clause.to_string(),
                file: "a.rs".to_string(),
                line: 1,
                code: "x".to_string(),
                problem: "p".to_string(),
                blocking: true,
                check_failed: None,
            })
            .collect(),
        exchanges: Vec::new(),
        closes: Vec::new(),
    }
}

/// Opens one branch per entry of <branches>, oldest first, and records each
/// entry's runs in order, with every finding confirmed.
fn store(branches: &[Vec<Vec<&str>>]) -> rusqlite::Connection {
    let db = db();
    let repo: String = db
        .query_row("SELECT name FROM repo LIMIT 1", [], |row| row.get(0))
        .unwrap();
    for (at, runs) in branches.iter().enumerate() {
        let landing = add_landing(&db, &format!("branch-{at}"), &repo, at as i64).unwrap();
        for clauses in runs {
            let run = NewRun {
                landing: Some(landing),
                kind: Kind::Review { changed_lines: 0 },
                label: None,
                at: at as i64,
                base: "base",
                head: "head",
                since: None,
                grouping: "g",
                rules_version: "",
            };
            let (_, ids) = add_run(&db, &run, &[call(clauses)]).unwrap();
            for id in ids.concat() {
                settle(&db, id, "confirmed", "real", at as i64, "tester").unwrap();
            }
        }
    }
    db
}

fn shown(db: &rusqlite::Connection) -> String {
    section(db, "demo", &rules()).unwrap()
}

#[test]
fn a_kind_found_only_on_first_reviews_older_than_the_last_branches_is_not_shown() {
    let mut branches = vec![vec![vec!["Name the actor"; 5]]];
    for _ in 0..BRANCHES {
        branches.push(vec![vec!["Use the words"]]);
    }
    branches
        .last_mut()
        .unwrap()
        .push(vec!["Name the actor and the action."; 5]);
    let text = shown(&store(&branches));
    assert!(
        text.contains("Literal. Use the words for what happens."),
        "{text}"
    );
    assert!(!text.contains("Active."), "{text}");
}

#[test]
fn a_kind_that_comes_back_on_a_recent_first_review_is_shown_again() {
    let mut branches = vec![vec![vec!["Name the actor"; 5]]];
    for _ in 0..BRANCHES {
        branches.push(vec![vec!["Use the words"]]);
    }
    branches.push(vec![vec!["Name the actor", "Name the actor"]]);
    let text = shown(&store(&branches));
    assert!(
        text.contains("plain, 2 findings: Active. Name the actor and the action."),
        "{text}"
    );
    assert!(text.contains("Literal."), "{text}");
}

/// A first review that confirmed <findings>, each a rule and a clause,
/// numbered from 1.
fn review(findings: &[(&str, &str)]) -> FirstReview {
    FirstReview {
        at: 0,
        changed_lines: Some(1),
        groups: vec!["g".to_string()],
        confirmed: findings
            .iter()
            .enumerate()
            .map(|(at, (rule, clause))| record::Finding {
                id: at as i64 + 1,
                run_id: 1,
                kind: "review".to_string(),
                landing_id: Some(1),
                group: "g".to_string(),
                rule: rule.to_string(),
                clause: clause.to_string(),
                file: "a.rs".to_string(),
                line: 1,
                code: "x".to_string(),
                problem: "p".to_string(),
                blocking: true,
                check_failed: None,
                settled: Some(record::Settled::Once(record::Settle {
                    how: "confirmed".to_string(),
                    why: "real".to_string(),
                    by: "tester".to_string(),
                })),
                closed_by: None,
            })
            .collect(),
    }
}

#[test]
fn a_kind_is_the_paragraph_or_list_item_that_holds_the_clause() {
    let reviews = [review(&[
        ("plain", "Name what the code does."),
        ("plain", "a clause no rule holds"),
        ("gone", "Name the actor"),
    ])];
    let counted = frequent(&reviews.iter().collect::<Vec<_>>(), &rules());
    assert_eq!(counted.kinds.len(), 1, "{counted:?}");
    assert_eq!(counted.kinds[0].text, "Name what the code does.");
    assert_eq!(counted.kinds[0].findings, 1);
    assert_eq!(
        counted.left_out,
        [
            "WHAT REVIEWS CONFIRM MOST OFTEN leaves out finding 2, whose clause no paragraph of rule plain holds",
            "WHAT REVIEWS CONFIRM MOST OFTEN leaves out finding 3, whose rule gone the rules checkout no longer holds",
        ]
    );
}

#[test]
fn newer_first_reviews_count_more() {
    let mut reviews = vec![review(&[("plain", "Use the words")])];
    reviews.extend(vec![review(&[]); 2 * HALF_LIFE as usize - 1]);
    reviews.push(review(&[("plain", "Name the actor"); 2]));
    let counted = frequent(&reviews.iter().collect::<Vec<_>>(), &rules());
    assert_eq!(
        counted.kinds[0].text,
        "Literal. Use the words for what happens."
    );
    assert_eq!(
        counted.kinds.len(),
        1,
        "frequent leaves out a kind whose findings weigh less than one: {counted:?}"
    );
}
