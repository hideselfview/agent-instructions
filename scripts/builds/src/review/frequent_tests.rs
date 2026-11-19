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
    section(db, &[], "demo", &rules()).unwrap()
}

#[test]
fn a_paragraph_quoted_only_on_first_reviews_older_than_the_last_branches_is_not_shown() {
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
fn a_paragraph_that_comes_back_on_a_recent_first_review_is_shown_again() {
    let mut branches = vec![vec![vec!["Name the actor"; 5]]];
    for _ in 0..BRANCHES {
        branches.push(vec![vec!["Use the words"]]);
    }
    branches.push(vec![vec!["Name the actor", "Name the actor"]]);
    let text = shown(&store(&branches));
    assert!(
        text.contains("    2x Active. Name the actor and the action."),
        "{text}"
    );
    assert!(
        text.contains("    29x Literal. Use the words for what happens."),
        "{text}"
    );
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
                    at: 0,
                })),
                closed_by: None,
            })
            .collect(),
    }
}

#[test]
fn a_rule_counts_its_findings_across_paragraphs_and_names_the_most_quoted_first() {
    let reviews = [review(&[
        ("plain", "Name what the code does."),
        ("plain", "Name the actor"),
        ("plain", "Use the words"),
        ("plain", "Name the actor"),
        ("plain", "a clause no paragraph holds any more"),
        ("gone", "Name the actor"),
    ])];
    let counted = frequent(&reviews.iter().collect::<Vec<_>>(), &rules());
    assert_eq!(counted.rules.len(), 1, "{counted:?}");
    let plain = &counted.rules[0];
    assert_eq!(plain.rule, "plain");
    assert_eq!(plain.findings, 5);
    assert_eq!(
        plain
            .quoted
            .iter()
            .map(|quoted| (quoted.text.as_str(), quoted.findings))
            .collect::<Vec<_>>(),
        [
            ("Active. Name the actor and the action.", 2),
            ("Name what the code does.", 1),
            ("Literal. Use the words for what happens.", 1),
        ]
    );
    assert_eq!(
        counted.left_out,
        [
            "WHAT REVIEWS CONFIRM MOST OFTEN counts finding 5 under rule plain but lists no paragraph for it, since no paragraph of the rule holds its clause",
            "WHAT REVIEWS CONFIRM MOST OFTEN leaves out finding 6, whose rule gone the rules checkout no longer holds",
        ]
    );
}

#[test]
fn a_rule_with_one_finding_is_not_listed() {
    let reviews = [review(&[("plain", "Name the actor")])];
    let counted = frequent(&reviews.iter().collect::<Vec<_>>(), &rules());
    assert_eq!(counted.rules, []);
}

#[test]
fn a_rule_with_newer_findings_comes_first() {
    let other = RULE.replace("Write plainly", "Other");
    let rules = vec![Rule::parse("plain", RULE), Rule::parse("other", &other)];
    let mut reviews = vec![review(&[("other", "Name the actor"); 2])];
    reviews.extend(vec![review(&[]); HALF_LIFE as usize]);
    reviews.push(review(&[("plain", "Name the actor"); 2]));
    reviews.reverse();
    let counted = frequent(&reviews.iter().collect::<Vec<_>>(), &rules);
    assert_eq!(
        counted
            .rules
            .iter()
            .map(|r| r.rule.as_str())
            .collect::<Vec<_>>(),
        ["plain", "other"]
    );
}

#[test]
fn other_projects_count_only_findings_of_shared_rules() {
    let mut shared = Rule::parse("plain", RULE);
    shared.path = "rules/text/plain.md".to_string();
    let mut own = Rule::parse("mine", RULE);
    own.path = "projects/demo-rules/mine.md".to_string();
    let rules = vec![shared, own];
    let from_other = vec![review(&[
        ("plain", "Name the actor"),
        ("mine", "Name the actor"),
    ])];
    let merged = counted_reviews(Vec::new(), vec![from_other], &rules);
    assert_eq!(merged.len(), 1);
    assert_eq!(
        merged[0]
            .confirmed
            .iter()
            .map(|f| f.rule.as_str())
            .collect::<Vec<_>>(),
        ["plain"]
    );
    let own_store = vec![review(&[("mine", "Name the actor")])];
    let merged = counted_reviews(own_store, Vec::new(), &rules);
    assert_eq!(merged[0].confirmed.len(), 1);
}
