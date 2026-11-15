use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{Fake, fake};
use crate::review::{Inputs as ReviewInputs, review};
use crate::testing::git as run_git;

fn stamp() -> Stamp {
    Stamp {
        at: NOON,
        by: "tester".to_string(),
    }
}

/// A reviewed change of a.rs and b.swift whose rule-a finding the coordinator
/// confirmed and whose rule-b finding it dropped; returns the landing.
fn reviewed(demo: &Demo, fake: &Fake, side: &Path) -> i64 {
    demo.commit(side, "a.rs");
    demo.commit(side, "b.swift");
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some(("Never do the bad thing here.", "a.rs", 1, "a.rs", "p")),
        )],
    );
    fake.answer(
        "beta",
        &[(
            "rule-b",
            Some(("Never do the other thing.", "b.swift", 1, "b.swift", "p")),
        )],
    );
    let landing = review(&ReviewInputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: side,
        base: "main",
        head: None,
        key: "k",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        brief: None,
    })
    .unwrap()
    .landing
    .unwrap();
    let conn = demo.store.open_existing().unwrap();
    let findings = record::findings_of_landing(&conn, landing).unwrap();
    drop(conn);
    let id_of = |rule: &str| findings.iter().find(|f| f.rule == rule).unwrap().id;
    demo.store
        .write(&stamp(), "settle", |db| {
            record::settle(db, id_of("rule-a"), "confirmed", "real", NOON, "tester")?;
            record::settle(db, id_of("rule-b"), "dropped", "wrong", NOON, "tester")
        })
        .unwrap();
    fake.prompts();
    landing
}

fn replay_of(demo: &Demo, fake: &Fake, side: &Path, landing: i64, groups: &[&str]) -> Replayed {
    replay(&Inputs {
        store: &demo.store,
        project: "demo",
        repo_dir: side,
        landings: &[landing],
        root: &fake.root,
        reviewer: &fake.program,
        model: Some("haiku"),
        groups,
        stamp: &stamp(),
    })
    .unwrap()
}

#[test]
fn a_replay_of_a_landing_scores_its_findings_against_the_settled_ones_beside_the_review() {
    let demo = demo("review-rerun");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let landing = reviewed(&demo, &fake, &side);
    // The replay misses the confirmed finding and raises the dropped one again.
    fake.answer("alpha", &[("rule-a", None)]);
    let replayed = replay_of(&demo, &fake, &side, landing, &[]);
    let text = &replayed.text;
    assert!(!replayed.failed, "{text}");
    assert!(text.contains("2 settled findings to pair, 0 new"), "{text}");
    assert!(
        text.contains("in use  sonnet, prompt ") && text.contains("replay  haiku, prompt "),
        "the replay asks every group in the model given: {text}"
    );
    assert!(
        text.contains("    rule-a\n      in use  TP 1, FP 0, FN 0, TN 0, unsettled 0; precision 100%, recall 100%\n      replay  TP 0, FP 0, FN 1, TN 0, new 0; precision n/a, recall 0%\n"),
        "{text}"
    );
    assert!(
        text.contains("    rule-b\n      in use  TP 0, FP 1, FN 0, TN 0, unsettled 0; precision 0%, recall n/a\n      replay  TP 0, FP 1, FN 0, TN 0, new 0; precision 0%, recall n/a\n"),
        "{text}"
    );
    let conn = demo.store.open_existing().unwrap();
    let replays = record::calls_of_kind(&conn, Kind::Replay).unwrap();
    assert_eq!(replays.len(), 2, "the replay is recorded");
    assert!(replays.iter().all(|c| c.model == "haiku"));
    drop(conn);
    // Asked for one group, the replay asks it alone and scores only its rules.
    fake.prompts();
    let replayed = replay_of(&demo, &fake, &side, landing, &["beta"]);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].0, "beta");
    assert!(
        !replayed.text.contains("rule-a") && replayed.text.contains("rule-b"),
        "{}",
        replayed.text
    );
}

#[test]
fn a_run_whose_commits_the_repo_lost_is_found_by_its_patch_ids_on_the_history() {
    let demo = demo("review-rerun-landed");
    let side = demo.add_side(1, "agent", "work");
    let first = demo.commit(&side, "a.rs");
    let second = demo.commit(&side, "b.rs");
    let read = commits::with_patch_ids(&side, &[first.clone(), second.clone()]).unwrap();
    // main moves on, so the change lands on another parent with new SHAs.
    demo.commit(&demo.main, "m.txt");
    run_git(&demo.main, &["cherry-pick", &first, &second]);
    let (base, head) = landed(&demo.main, &read).unwrap();
    let main_head = String::from_utf8(
        crate::checkout::git(&demo.main)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(head, main_head.trim());
    assert_ne!(head, second, "the cherry-picks are new commits");
    let diff = run::diff_between(&demo.main, &base, &head).unwrap();
    assert_eq!(diff.changed, ["a.rs", "b.rs"]);
    let unknown = [Commit {
        sha: "0".repeat(40),
        patch_id: "f".repeat(40),
    }];
    assert!(
        landed(&demo.main, &unknown)
            .unwrap_err()
            .text()
            .contains("no run of commits with the run's patch ids")
    );
}
