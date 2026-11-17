use super::*;
use crate::db;
use crate::lists::Stamp;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::demo;
use crate::review::fakes::fake;
use crate::review::{Inputs as ReviewInputs, audit, review};

fn call(id: i64, run: i64, group: &str, rules: &[&str], cost: Option<i64>) -> Call {
    Call {
        id,
        run_id: run,
        kind: "review".to_string(),
        group: group.to_string(),
        model: "opus".to_string(),
        prompt: format!("prompt-of-{group}"),
        rules: rules.iter().map(|r| r.to_string()).collect(),
        tokens_in: 100,
        tokens_out: 10,
        tokens_cached: 0,
        tokens_written: 0,
        cost_microdollars: cost,
        failure: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn finding(
    id: i64,
    run: i64,
    group: &str,
    rule: &str,
    file: &str,
    line: i64,
    code: &str,
    settled: Option<&str>,
) -> Finding {
    Finding {
        id,
        run_id: run,
        kind: "review".to_string(),
        landing_id: Some(1),
        group: group.to_string(),
        rule: rule.to_string(),
        clause: "c".to_string(),
        file: file.to_string(),
        line,
        code: code.to_string(),
        problem: "p".to_string(),
        blocking: true,
        check_failed: None,
        settled: settled.map(|how| {
            record::Settled::Once(record::Settle {
                how: how.to_string(),
                why: "why".to_string(),
                by: "tester".to_string(),
            })
        }),
        closed_by: None,
    }
}

fn verdict(call: i64, rule: &str, verdict: &str) -> Verdict {
    Verdict {
        call_id: call,
        rule: rule.to_string(),
        verdict: verdict.to_string(),
    }
}

fn run(id: i64, kind: &str, head: &str) -> RunRow {
    RunRow {
        id,
        kind: kind.to_string(),
        base: "b".to_string(),
        head: head.to_string(),
        since: None,
        grouping: "g".to_string(),
        rules_version: "abc".to_string(),
    }
}

fn tally_of(scores: &Scores, group: &str, rule: &str) -> Tally {
    scores
        .groups
        .iter()
        .find(|g| g.setup.group == group)
        .and_then(|g| g.rules.iter().find(|(r, _)| r == rule))
        .map(|(_, t)| *t)
        .unwrap_or_else(|| panic!("no counts for {group} {rule}: {scores:?}"))
}

fn tally(tp: usize, fp: usize, fn_: usize, tn: usize, unsettled: usize) -> Tally {
    Tally {
        true_pos: tp,
        false_pos: fp,
        false_neg: fn_,
        true_neg: tn,
        unsettled,
    }
}

#[test]
fn a_settled_finding_pairs_by_rule_file_and_code_before_lines_near_it() {
    let truth = finding(1, 1, "g", "r", "src/a.rs", 10, "let x = one();", None);
    let near_only = finding(2, 2, "g", "r", "a.rs", 11, "something else", None);
    let same_code = finding(3, 2, "g", "r", "a.rs", 60, "x = one()", None);
    assert_eq!(
        pair(&[&truth], &[&near_only, &same_code]),
        [Some(1)],
        "the code names the site before a near line does"
    );
    let second = finding(4, 1, "g", "r", "a.rs", 13, "unquoted", None);
    assert_eq!(
        pair(&[&truth, &second], &[&near_only, &same_code]),
        [Some(1), Some(0)],
        "each raised finding pairs once, and a line within three pairs"
    );
    let far = finding(5, 2, "g", "r", "a.rs", 14 + NEAR_LINES, "elsewhere", None);
    let other_rule = finding(6, 2, "g", "q", "a.rs", 10, "let x = one();", None);
    let other_file = finding(7, 2, "g", "r", "b.rs", 10, "let x = one();", None);
    assert_eq!(pair(&[&truth], &[&far, &other_rule, &other_file]), [None]);
}

#[test]
fn settled_findings_count_as_true_and_false_positives_and_a_confirmed_miss_as_a_false_negative() {
    let runs = [run(1, "review", "h"), run(2, "audit", "h")];
    let calls = [
        call(10, 1, "words", &["r1", "r2"], Some(500_000)),
        call(11, 1, "reads", &["r3"], None),
        call(12, 1, "types", &["r4"], Some(250_000)),
    ];
    let verdicts = [
        verdict(10, "r1", "finding"),
        verdict(10, "r2", "finding"),
        verdict(11, "r3", "none"),
        verdict(12, "r4", "none"),
    ];
    let findings = [
        finding(1, 1, "words", "r1", "a.rs", 1, "x", Some("confirmed")),
        finding(2, 1, "words", "r1", "a.rs", 9, "y", Some("dropped")),
        finding(3, 1, "words", "r2", "a.rs", 3, "z", None),
    ];
    let mut miss = finding(4, 2, "audit r4", "r4", "b.rs", 1, "m", Some("confirmed"));
    miss.kind = "audit".to_string();
    let mut unasked = finding(5, 2, "audit r5", "r5", "b.rs", 2, "n", Some("confirmed"));
    unasked.kind = "audit".to_string();
    let mut scores = Scores::default();
    count_settled(
        &mut scores,
        &runs,
        &calls.iter().collect::<Vec<_>>(),
        &verdicts,
        &findings.iter().collect::<Vec<_>>(),
        &[&miss, &unasked],
    );
    assert_eq!(tally_of(&scores, "words", "r1"), tally(1, 1, 0, 0, 0));
    assert_eq!(tally_of(&scores, "words", "r2"), tally(0, 0, 0, 0, 1));
    assert_eq!(tally_of(&scores, "reads", "r3"), tally(0, 0, 0, 1, 0));
    assert_eq!(
        tally_of(&scores, "types", "r4"),
        tally(0, 0, 1, 0, 0),
        "a \"none\" over a diff with a confirmed miss in the rule is no true negative"
    );
    assert_eq!(
        tally_of(&scores, "(no group answered for the rule)", "r5"),
        tally(0, 0, 1, 0, 0)
    );
    let words = scores
        .groups
        .iter()
        .find(|g| g.setup.group == "words")
        .unwrap();
    assert_eq!(words.total().precision(), Some(0.5));
    assert_eq!(words.total().recall(), Some(1.0));
    assert_eq!(words.rules_versions, ["abc"]);
    let reads = scores
        .groups
        .iter()
        .find(|g| g.setup.group == "reads")
        .unwrap();
    assert_eq!((reads.spend.calls, reads.spend.uncosted), (1, 1));
    assert!(spend_text(&reads.spend).contains("cost not recorded"));
    assert_eq!(reads.total().precision(), None);
}

#[test]
fn a_replay_scores_the_settled_findings_it_raises_again_and_names_the_ones_it_adds() {
    let truths = [
        finding(1, 1, "words", "r1", "a.rs", 10, "bad()", Some("confirmed")),
        finding(
            2,
            1,
            "words",
            "r1",
            "a.rs",
            30,
            "worse()",
            Some("confirmed"),
        ),
        finding(3, 1, "words", "r2", "b.rs", 5, "fine()", Some("dropped")),
        finding(4, 1, "words", "r2", "b.rs", 50, "ok()", Some("dropped")),
    ];
    let calls = [call(20, 9, "words", &["r1", "r2", "r3"], Some(100_000))];
    let verdicts = [
        verdict(20, "r1", "finding"),
        verdict(20, "r2", "finding"),
        verdict(20, "r3", "none"),
    ];
    let mut quote_not_in_diff = finding(13, 9, "words", "r1", "a.rs", 30, "worse()", None);
    quote_not_in_diff.check_failed = Some("its code is not in the diff of a.rs".to_string());
    let raised = [
        finding(10, 9, "words", "r1", "a.rs", 11, "bad()", None),
        finding(11, 9, "words", "r2", "b.rs", 6, "fine();", None),
        finding(12, 9, "words", "r1", "c.rs", 1, "new()", None),
        quote_not_in_diff,
    ];
    let mut scores = Scores::default();
    let new = count_replay(
        &mut scores,
        &calls,
        &verdicts,
        &raised,
        &truths.iter().collect::<Vec<_>>(),
        "abc",
    );
    assert_eq!(new.iter().map(|f| f.id).collect::<Vec<_>>(), [12]);
    assert_eq!(tally_of(&scores, "words", "r1"), tally(1, 0, 1, 0, 1));
    assert_eq!(tally_of(&scores, "words", "r2"), tally(0, 1, 0, 0, 0));
    assert_eq!(tally_of(&scores, "words", "r3"), tally(0, 0, 0, 1, 0));
    // A call that failed judged nothing: its rules' truths count nowhere.
    let mut failed = calls.clone();
    failed[0].failure = Some("boom".to_string());
    let mut scores = Scores::default();
    count_replay(
        &mut scores,
        &failed,
        &verdicts,
        &[],
        &truths.iter().collect::<Vec<_>>(),
        "abc",
    );
    assert_eq!(scores.groups[0].total(), Tally::default());
    assert_eq!(scores.groups[0].spend.calls, 1);
}

/// The fake's rules plus rule-c, a second rule of group alpha.
fn add_rule_c(root: &std::path::Path) {
    std::fs::write(
        root.join("rules/alpha/rule-c.md"),
        "---\npaths:\n  - '**/*.rs'\nblocking: true\n---\n\nNever do the third thing.\n",
    )
    .unwrap();
}

#[test]
fn the_store_scores_each_setup_from_settled_findings_and_confirmed_audit_misses() {
    let demo = demo("review-scores");
    let fake = fake(&demo.scratch.dir);
    add_rule_c(&fake.root);
    let side = demo.add_side(1, "agent", "work");
    for file in ["a.rs", "b.swift", "c.rs"] {
        demo.commit(&side, file);
    }
    let bad = (
        "Never do the bad thing here.",
        "a.rs",
        1,
        "a.rs",
        "The file does the bad thing.",
    );
    fake.answer("alpha", &[("rule-a", Some(bad)), ("rule-c", None)]);
    fake.answer(
        "beta",
        &[(
            "rule-b",
            Some(("Never do the other thing.", "b.swift", 1, "b.swift", "p")),
        )],
    );
    let stamp = Stamp {
        at: NOON,
        by: "tester".to_string(),
    };
    let landing = review(&ReviewInputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: &side,
        base: "main",
        head: None,
        key: "k",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp,
        brief: None,
    })
    .unwrap()
    .landing
    .unwrap();
    let findings = {
        let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
        record::findings_of_landing(&conn, landing).unwrap()
    };
    let id_of = |rule: &str| findings.iter().find(|f| f.rule == rule).unwrap().id;
    demo.store
        .write(&stamp, "settle", |db| {
            record::settle(db, id_of("rule-a"), "confirmed", "real", NOON, "tester")?;
            record::settle(db, id_of("rule-b"), "dropped", "wrong", NOON, "tester")?;
            let run = record::runs_of_landing(db, landing)?[0].id;
            record::record_landed(db, landing, run, NOON)
        })
        .unwrap();
    // The audit raises rule-a in c.rs, which the review did not.
    fake.answer(
        "audit rule-a",
        &[(
            "rule-a",
            Some(("Never do the bad thing here.", "c.rs", 1, "c.rs", "p")),
        )],
    );
    fake.answer("audit rule-b", &[("rule-b", None)]);
    fake.answer("audit rule-c", &[("rule-c", None)]);
    for shuffle in ["shuffle 1", "shuffle 2"] {
        fake.answer(shuffle, &[]);
    }
    audit::audit(&audit::Inputs {
        store: &demo.store,
        project: "demo",
        repo_dir: &side,
        landing,
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp,
        seed: 3,
    })
    .unwrap();
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let all = record::findings_of_landing(&conn, landing).unwrap();
    let miss = audit::candidate_misses(&all)
        .into_iter()
        .find(|f| f.file == "c.rs")
        .unwrap()
        .id;
    drop(conn);
    demo.store
        .write(&stamp, "settle", |db| {
            record::settle(db, miss, "confirmed", "a real miss", NOON, "tester")
        })
        .unwrap();
    let conn = db::open_reader(&demo.store.database()).unwrap().unwrap();
    let scores = of_store(&conn).unwrap();
    assert_eq!((scores.landings, scores.review_runs), (1, 1));
    assert_eq!(
        tally_of(&scores.reviews, "alpha", "rule-a"),
        tally(1, 0, 1, 0, 0)
    );
    assert_eq!(
        tally_of(&scores.reviews, "alpha", "rule-c"),
        tally(0, 0, 0, 1, 0)
    );
    assert_eq!(
        tally_of(&scores.reviews, "beta", "rule-b"),
        tally(0, 1, 0, 0, 0)
    );
    let alpha = scores
        .reviews
        .groups
        .iter()
        .find(|g| g.setup.group == "alpha")
        .unwrap();
    assert_eq!(alpha.setup.model, "sonnet");
    assert_eq!(alpha.spend.calls, 1);
    assert_eq!(alpha.spend.cost_microdollars, 10_000);
    assert!(scores.audits.calls >= 3);
    let report = store_report("demo", &scores);
    assert!(
        report.contains("rule-a: TP 1, FP 0, FN 1, TN 0, unsettled 0; precision 100%, recall 50%"),
        "{report}"
    );
    assert!(
        report.contains("rule-b: TP 0, FP 1, FN 0, TN 0, unsettled 0; precision 0%, recall n/a"),
        "{report}"
    );
    assert!(report.contains("sonnet, prompt "), "{report}");
    assert!(report.contains("audits: "), "{report}");
}

/// A finding the coordinator settled again as dropped counts as an FP, and a
/// confirmed finding that a later review closed stays a TP.
#[test]
fn a_finding_settled_again_counts_as_dropped_and_a_closed_one_as_confirmed() {
    let db = crate::lists::fixtures::db();
    let landing = record::add_landing(&db, "k", "bae", 5).unwrap();
    let raised = |code: &str| record::NewFinding {
        rule: "r".to_string(),
        clause: "c".to_string(),
        file: "a.rs".to_string(),
        line: 1,
        code: code.to_string(),
        problem: "p".to_string(),
        blocking: true,
        check_failed: None,
    };
    let call = record::NewCall {
        group: "words".to_string(),
        model: "sonnet".to_string(),
        prompt: "the prompt".to_string(),
        rules: vec!["r".to_string()],
        tokens_in: 0,
        tokens_out: 0,
        tokens_cached: 0,
        tokens_written: 0,
        cost_microdollars: 0,
        sites: None,
        failure: None,
        verdicts: vec![("r".to_string(), "finding".to_string())],
        findings: vec![raised("wrong"), raised("gone")],
        closes: Vec::new(),
        exchanges: Vec::new(),
    };
    let new_run = record::NewRun {
        landing: Some(landing),
        kind: Kind::Review,
        label: None,
        at: 6,
        base: "b",
        head: "h",
        since: None,
        grouping: "g",
        rules_version: "",
    };
    let (_, ids) = record::add_run(&db, &new_run, std::slice::from_ref(&call)).unwrap();
    let (wrong, gone) = (ids[0][0], ids[0][1]);
    for id in [wrong, gone] {
        record::settle(&db, id, "confirmed", "real", 7, "me").unwrap();
    }
    record::settle_again(&db, wrong, "another crate uses it", 8, "me").unwrap();
    let closing = record::NewCall {
        rules: Vec::new(),
        verdicts: Vec::new(),
        findings: Vec::new(),
        closes: vec![gone],
        ..call
    };
    record::add_run(
        &db,
        &record::NewRun {
            head: "h2",
            ..new_run
        },
        &[closing],
    )
    .unwrap();
    let scores = of_store(&db).unwrap();
    assert_eq!(
        tally_of(&scores.reviews, "words", "r"),
        tally(1, 1, 0, 0, 0)
    );
}
