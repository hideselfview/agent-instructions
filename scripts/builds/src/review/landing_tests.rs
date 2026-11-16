use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{Fake, Raised, fake};
use crate::review::{Inputs, review};

const BAD: Raised = (
    "Never do the bad thing here.",
    "a.rs",
    1,
    "a.rs",
    "The file does the bad thing.",
);

fn stamp() -> Stamp {
    Stamp {
        at: NOON,
        by: "tester".to_string(),
    }
}

fn review_of(demo: &Demo, fake: &Fake, side: &Path) -> super::super::Reported {
    review_past(demo, fake, side, "main")
}

fn review_past(demo: &Demo, fake: &Fake, side: &Path, base: &str) -> super::super::Reported {
    review(&Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: side,
        base,
        head: None,
        key: "branch:work",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        brief: None,
    })
    .unwrap()
}

/// What `builds land` would ask of <shas> in <side>: the verdict and its
/// words.
fn verdict_of(demo: &Demo, fake: &Fake, side: &Path, shas: &[String]) -> (Verdict, Option<String>) {
    let asked = asked(demo, fake, side, shas);
    let verdict = judge(&asked.conn, "demo", &asked.needs).unwrap();
    let words = refusal(&asked.commits(), &verdict);
    (verdict, words)
}

fn asked(demo: &Demo, fake: &Fake, side: &Path, shas: &[String]) -> Asked {
    let mut words: Vec<String> = vec![side.display().to_string()];
    words.extend(shas.iter().cloned());
    words.extend([
        "--project".to_string(),
        "demo".to_string(),
        "--rules-root".to_string(),
        fake.root.display().to_string(),
    ]);
    let args = Args::parse(&words, &["project", "rules-root"], &[]).unwrap();
    ask_of(&demo.tool, &args)
}

fn ask_of(tool: &Tool, args: &Args) -> Asked {
    super::ask(tool, args).unwrap()
}

fn findings(demo: &Demo) -> Vec<record::Finding> {
    let conn = demo.store.open_existing().unwrap();
    let landing = record::open_landing(&conn, "branch:work").unwrap().unwrap();
    record::findings_of_landing(&conn, landing).unwrap()
}

fn settle(demo: &Demo, finding: i64, how: &str) {
    let stamp = stamp();
    demo.store
        .write(&stamp, "settle", |db| {
            record::settle(db, finding, how, "judged", stamp.at, "tester")
        })
        .unwrap();
}

fn fix(demo: &Demo, side: &Path, finding: i64, sha: &str) {
    let commit = fix_commit(side, sha).unwrap();
    let stamp = stamp();
    demo.store
        .write(&stamp, "fixed", |db| {
            record::record_fix(db, finding, &commit, stamp.at, "tester")
        })
        .unwrap();
}

fn refused(verdict: &(Verdict, Option<String>)) -> &str {
    verdict.1.as_deref().expect("the review refuses")
}

#[test]
fn commits_no_review_read_are_refused_and_commits_no_group_covers_need_none() {
    let demo = demo("landing-no-run");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    let asked = verdict_of(&demo, &fake, &side, std::slice::from_ref(&a));
    assert_eq!(asked.0, Verdict::NoRun);
    let words = refused(&asked);
    assert!(
        words.contains("no review read the commits this lands") && words.contains(ONE_REVIEW),
        "{words}"
    );
    let notes = demo.commit(&side, "notes.txt");
    assert_eq!(
        verdict_of(&demo, &fake, &side, &[notes]).0,
        Verdict::NotNeeded,
        "no rule covers a .txt file"
    );
}

#[test]
fn an_unsettled_finding_is_refused_and_a_settled_review_takes_the_commits() {
    let demo = demo("landing-unsettled");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    review_of(&demo, &fake, &side);
    let asked = verdict_of(&demo, &fake, &side, std::slice::from_ref(&a));
    assert!(matches!(asked.0, Verdict::Unfinished(..)));
    assert!(
        refused(&asked).contains("1 finding is not settled"),
        "{asked:?}"
    );
    settle(&demo, findings(&demo)[0].id, "dropped");
    let asked = verdict_of(&demo, &fake, &side, &[a]);
    assert!(matches!(asked.0, Verdict::Takes(_)), "{asked:?}");
    assert_eq!(asked.1, None);
}

#[test]
fn a_named_fix_lands_with_the_reviewed_commits_and_no_review_of_its_own() {
    let demo = demo("landing-fix");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    review_of(&demo, &fake, &side);
    let finding = findings(&demo)[0].id;
    settle(&demo, finding, "confirmed");
    let asked = verdict_of(&demo, &fake, &side, std::slice::from_ref(&a));
    assert!(refused(&asked).contains("names no fix"), "{asked:?}");
    // The fix is a new commit, which the review never reads.
    let b = demo.commit(&side, "b.rs");
    let asked = verdict_of(&demo, &fake, &side, &[a.clone(), b.clone()]);
    assert!(
        refused(&asked).contains(&format!("commit {} is not one the review read", &b[..10])),
        "{asked:?}"
    );
    fix(&demo, &side, finding, &b);
    let asked = verdict_of(&demo, &fake, &side, std::slice::from_ref(&a));
    assert!(
        refused(&asked).contains("not among the commits this lands"),
        "{asked:?}"
    );
    fake.prompts();
    let asked = verdict_of(&demo, &fake, &side, &[a, b]);
    assert!(matches!(asked.0, Verdict::Takes(_)), "{asked:?}");
    assert!(fake.prompts().is_empty(), "nothing reviewed the fix");
}

#[test]
fn a_commit_added_after_the_review_lands_as_a_change_of_its_own() {
    let demo = demo("landing-new-work");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    let b = demo.commit(&side, "b.rs");
    review_of(&demo, &fake, &side);
    let c = demo.commit(&side, "c.rs");
    let said = verdict_of(&demo, &fake, &side, &[a.clone(), b.clone(), c.clone()]);
    let Verdict::Unfinished(_, unfinished) = &said.0 else {
        panic!("{said:?}")
    };
    assert_eq!(unfinished.unreviewed.len(), 1);
    let words = refused(&said);
    assert!(
        words.contains(&format!("commit {} is not one the review read", &c[..10]))
            && words.contains(ONE_REVIEW)
            && words.contains("review it as a change of its own"),
        "{words}"
    );
    // The reviewed commits land without it, every one of them or a part.
    assert!(matches!(
        verdict_of(&demo, &fake, &side, std::slice::from_ref(&a)).0,
        Verdict::Takes(_)
    ));
    let reviewed = asked(&demo, &fake, &side, &[a, b]);
    assert!(record_landed(&reviewed, &stamp()).unwrap().is_some());
    // The new commit is a change of its own: the branch's key opens a new
    // landing, and its review reads only the new commit.
    fake.prompts();
    let second = review_past(&demo, &fake, &side, &reviewed.needs[1].commit.sha);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].1.contains("+c.rs") && !prompts[0].1.contains("+a.rs"));
    assert!(second.refusal.is_none(), "{:?}", second.refusal);
    assert!(matches!(
        verdict_of(&demo, &fake, &side, &[c]).0,
        Verdict::Takes(_)
    ));
}

#[test]
fn a_rebased_commit_whose_added_lines_changed_is_a_new_diff() {
    let demo = demo("landing-rebased");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    demo.commit(&side, "b.rs");
    review_of(&demo, &fake, &side);
    // Another change in the last commit, as a rebase that resolves a conflict
    // makes.
    std::fs::write(side.join("b.rs"), "resolved\n").unwrap();
    crate::testing::git(&side, &["commit", "-q", "--amend", "-a", "--no-edit"]);
    let changed = crate::checkout::git_text(
        crate::checkout::git(&side).args(["rev-parse", "HEAD"]),
        "read HEAD",
    )
    .unwrap()
    .trim()
    .to_string();
    let asked = verdict_of(&demo, &fake, &side, &[a, changed.clone()]);
    let Verdict::Unfinished(_, unfinished) = &asked.0 else {
        panic!("{asked:?}")
    };
    assert_eq!(unfinished.unreviewed.len(), 1);
    assert_eq!(unfinished.unreviewed[0].sha, changed);
    assert!(refused(&asked).contains("one whose added or removed lines a rebase changed"));
}

#[test]
fn a_cherry_pick_onto_a_moved_main_keeps_the_review_and_landed_records_it_once() {
    let demo = demo("landing-picked");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    review_of(&demo, &fake, &side);
    // The same change on another branch, picked onto a main that moved.
    let other = demo.add_side(2, "agent", "picked");
    demo.commit(&other, "other.txt");
    crate::testing::git(&other, &["cherry-pick", &a]);
    let picked = crate::checkout::git_text(
        crate::checkout::git(&other).args(["rev-parse", "HEAD"]),
        "read HEAD",
    )
    .unwrap()
    .trim()
    .to_string();
    assert_ne!(picked, a);
    let asked_picked = asked(&demo, &fake, &other, std::slice::from_ref(&picked));
    let conn = demo.store.open_existing().unwrap();
    assert!(matches!(
        judge(&conn, "demo", &asked_picked.needs).unwrap(),
        Verdict::Takes(_)
    ));
    drop(conn);
    let landing = record_landed(&asked_picked, &stamp()).unwrap();
    assert!(landing.is_some());
    // Recording again finds the landed run and records nothing more.
    assert_eq!(record_landed(&asked_picked, &stamp()).unwrap(), None);
    let conn = demo.store.open_existing().unwrap();
    assert!(
        record::landing(&conn, landing.unwrap())
            .unwrap()
            .landed_run
            .is_some()
    );
    assert_eq!(record::open_landing(&conn, "branch:work").unwrap(), None);
}

#[test]
fn landed_records_the_worktrees_claim_wherever_it_runs() {
    let demo = demo("review-landing-who");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    review_of(&demo, &fake, &side);
    let words: Vec<String> = [
        side.display().to_string(),
        a,
        "--project".to_string(),
        "demo".to_string(),
        "--rules-root".to_string(),
        fake.root.display().to_string(),
    ]
    .into();
    let args = Args::parse(&words, &["project", "rules-root"], &[]).unwrap();
    // The test process runs in no claimed worktree, as builds land does when
    // the coordinator runs it from the person's checkout.
    assert!(record_landing(&demo.tool, &args).unwrap().is_some());
    let conn = demo.store.open_existing().unwrap();
    let log = crate::lists::store::read_log(&conn).unwrap();
    let (_, _, by, words) = log.last().unwrap();
    assert!(words.starts_with("review landed"), "{words}");
    assert_eq!(by, "agent");
}

#[test]
fn a_rename_out_of_a_covered_folder_is_reviewed_by_the_group_of_its_old_path() {
    let demo = demo("landing-rename");
    let fake = fake(&demo.scratch.dir);
    // Group gamma covers only src/, and its reviewer reads one file per call.
    std::fs::create_dir_all(fake.root.join("rules/gamma")).unwrap();
    std::fs::write(
        fake.root.join("rules/gamma/rule-g.md"),
        "---\ndigest: d\npaths:\n  - 'src/**'\nblocking: false\n---\n\n**Rule G.** Never do it in src.\n",
    )
    .unwrap();
    crate::review::fakes::group_file(
        &fake.root,
        "gamma",
        "gamma",
        "sonnet",
        "enumerate = true\nsites = \"handles rows\"\n",
    );
    fake.answer_task("gamma", serde_json::json!({"summary": "s", "sites": []}));
    std::fs::create_dir_all(demo.main.join("src")).unwrap();
    demo.commit(&demo.main, "src/a.kt");
    let side = demo.add_side(1, "agent", "work");
    std::fs::create_dir_all(side.join("tool")).unwrap();
    crate::testing::git(&side, &["mv", "src/a.kt", "tool/a.kt"]);
    crate::testing::git(&side, &["commit", "-q", "-m", "move a.kt out of src"]);
    let moved = crate::checkout::git_text(
        crate::checkout::git(&side).args(["rev-parse", "HEAD"]),
        "read HEAD",
    )
    .unwrap()
    .trim()
    .to_string();
    let reported = review_of(&demo, &fake, &side);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{}", reported.text);
    assert_eq!(prompts[0].0, "gamma");
    assert!(
        prompts[0].1.contains("rename from src/a.kt"),
        "the reviewer reads the rename: {}",
        prompts[0].1
    );
    assert!(reported.refusal.is_none(), "{:?}", reported.refusal);
    let asked = verdict_of(&demo, &fake, &side, &[moved]);
    assert!(matches!(asked.0, Verdict::Takes(_)), "{asked:?}");
}
