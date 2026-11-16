use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{Fake, Raised, fake};
use crate::review::files;
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
    let verdict = judge(&asked.conn, "demo", side, &asked.needs).unwrap();
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
            && words.contains("Run builds review in the branch's worktree"),
        "{words}"
    );
    // The reviewed commits land without it, every one of them or a part.
    assert!(matches!(
        verdict_of(&demo, &fake, &side, std::slice::from_ref(&a)).0,
        Verdict::Takes(_)
    ));
    let reviewed = asked(&demo, &fake, &side, &[a, b]);
    assert!(record_landed(&reviewed, &stamp(), false).unwrap().is_some());
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
    let changed = head(&side);
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
    let picked = head(&other);
    assert_ne!(picked, a);
    let asked_picked = asked(&demo, &fake, &other, std::slice::from_ref(&picked));
    let conn = demo.store.open_existing().unwrap();
    assert!(matches!(
        judge(&conn, "demo", &other, &asked_picked.needs).unwrap(),
        Verdict::Takes(_)
    ));
    drop(conn);
    let landing = record_landed(&asked_picked, &stamp(), false).unwrap();
    assert!(landing.is_some());
    // Recording again finds the landed run and records nothing more.
    assert_eq!(record_landed(&asked_picked, &stamp(), false).unwrap(), None);
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
    let moved = head(&side);
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

/// Commits <file>, holding <file>'s name, in <top> with <message>, and returns
/// the commit.
fn commit_with(top: &Path, file: &str, message: &str) -> String {
    std::fs::write(top.join(file), file).unwrap();
    crate::testing::git(top, &["add", file]);
    crate::testing::git(top, &["commit", "-q", "-m", message]);
    head(top)
}

/// What the reviewer of group alpha answers: rule-a's findings, each as
/// (file, code).
fn alpha_finds(fake: &Fake, found: &[(&str, &str)]) {
    let findings: Vec<serde_json::Value> = found
        .iter()
        .map(|(file, code)| {
            serde_json::json!({"clause": BAD.0, "file": file, "line": 1, "code": code, "problem": "bad"})
        })
        .collect();
    fake.answer_task(
        "alpha",
        serde_json::json!({"rules": [{"rule": "rule-a", "verdict": "finding", "findings": findings}]}),
    );
}

#[test]
fn a_branch_lands_in_two_parts_each_with_the_fixes_of_its_own_findings() {
    let demo = demo("landing-parts");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = commit_with(&side, "a.rs", "Add a");
    let b = commit_with(&side, "b.rs", "Add b with a vague line");
    alpha_finds(
        &fake,
        &[("a.rs", "a.rs"), (files::COMMIT_MESSAGE, "a vague line")],
    );
    review_of(&demo, &fake, &side);
    let found = findings(&demo);
    assert_eq!(found.len(), 2, "{found:?}");
    for finding in &found {
        settle(&demo, finding.id, "confirmed");
    }
    // The fix of the finding on a.rs changes a.rs.
    std::fs::write(side.join("a.rs"), "fixed\n").unwrap();
    crate::testing::git(&side, &["commit", "-q", "-a", "-m", "Fix a"]);
    let fa = head(&side);
    fix(&demo, &side, found[0].id, &fa);
    // A reworded commit keeps its patch id, so the test names b itself as the
    // fix of the finding on b's message.
    fix(&demo, &side, found[1].id, &b);
    // a without its fix waits on it.
    let alone = verdict_of(&demo, &fake, &side, &[a.clone(), b.clone()]);
    assert!(
        refused(&alone).contains("not among the commits this lands"),
        "{alone:?}"
    );
    // Part one: the finding on b's message is not on these commits.
    let first = asked(&demo, &fake, &side, &[a, fa]);
    let conn = demo.store.open_existing().unwrap();
    let verdict = judge(&conn, "demo", &side, &first.needs).unwrap();
    drop(conn);
    assert!(matches!(verdict, Verdict::Takes(_)), "{verdict:?}");
    assert_eq!(
        record_landed(&first, &stamp(), true).unwrap(),
        None,
        "b waits to land, so the landing stays open"
    );
    // Part two: the finding on a.rs belongs to part one.
    let second = verdict_of(&demo, &fake, &side, std::slice::from_ref(&b));
    assert!(matches!(second.0, Verdict::Takes(_)), "{second:?}");
    let second = asked(&demo, &fake, &side, &[b]);
    assert!(record_landed(&second, &stamp(), false).unwrap().is_some());
}

/// Names <sha> as the fix of <finding>, and returns why the store refused.
fn fix_refused(demo: &Demo, side: &Path, finding: i64, sha: &str) -> String {
    let commit = fix_commit(side, sha).unwrap();
    let stamp = stamp();
    demo.store
        .write(&stamp, "fixed", |db| {
            record::record_fix(db, finding, &commit, stamp.at, "tester")
        })
        .unwrap_err()
        .text()
}

/// The commit HEAD names in <side>.
fn head(side: &Path) -> String {
    crate::checkout::git_text(
        crate::checkout::git(side).args(["rev-parse", "HEAD"]),
        "read HEAD",
    )
    .unwrap()
    .trim()
    .to_string()
}

#[test]
fn a_finding_on_a_commit_a_part_landing_left_takes_its_fix_after_the_part_lands() {
    let demo = demo("landing-part-fix");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = commit_with(&side, "a.rs", "Add a");
    let b = commit_with(&side, "b.rs", "Add b");
    alpha_finds(&fake, &[("a.rs", "a.rs"), ("b.rs", "b.rs")]);
    review_of(&demo, &fake, &side);
    let found = findings(&demo);
    assert_eq!(found.len(), 2, "{found:?}");
    for finding in &found {
        settle(&demo, finding.id, "confirmed");
    }
    let (on_a, on_b) = if found[0].file == "a.rs" {
        (found[0].id, found[1].id)
    } else {
        (found[1].id, found[0].id)
    };
    std::fs::write(side.join("a.rs"), "fixed\n").unwrap();
    crate::testing::git(&side, &["commit", "-q", "-a", "-m", "Fix a"]);
    let fa = head(&side);
    fix(&demo, &side, on_a, &fa);
    let first = asked(&demo, &fake, &side, &[a, fa]);
    assert_eq!(record_landed(&first, &stamp(), true).unwrap(), None);
    // The part did not land b, so the store now takes a fix for the finding
    // on b.
    std::fs::write(side.join("b.rs"), "fixed\n").unwrap();
    crate::testing::git(&side, &["commit", "-q", "-a", "-m", "Fix b"]);
    let fb = head(&side);
    fix(&demo, &side, on_b, &fb);
    // The fix that landed stays.
    let refused = fix_refused(&demo, &side, on_a, &fb);
    assert!(
        refused.contains("a push put it on origin/main already"),
        "{refused}"
    );
    let second = verdict_of(&demo, &fake, &side, &[b.clone(), fb.clone()]);
    assert!(matches!(second.0, Verdict::Takes(_)), "{second:?}");
    let second = asked(&demo, &fake, &side, &[b, fb.clone()]);
    assert!(record_landed(&second, &stamp(), false).unwrap().is_some());
    let refused = fix_refused(&demo, &side, on_b, &fb);
    assert!(
        refused.contains("belongs to a change that landed already"),
        "{refused}"
    );
}

#[test]
fn a_finding_on_the_message_of_a_commit_an_earlier_landing_pushed_needs_no_fix_here() {
    let demo = demo("landing-pushed-message");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = commit_with(&side, "a.rs", "Add a with a vague line");
    review_of(&demo, &fake, &side);
    let first = asked(&demo, &fake, &side, std::slice::from_ref(&a));
    assert!(record_landed(&first, &stamp(), false).unwrap().is_some());
    // A cherry-pick put a on main, so a later review of the branch reads it
    // again beside the new commit.
    let b = commit_with(&side, "b.rs", "Add b");
    alpha_finds(&fake, &[(files::COMMIT_MESSAGE, "a vague line")]);
    review_of(&demo, &fake, &side);
    let found = findings(&demo);
    assert_eq!(found.len(), 1, "{found:?}");
    settle(&demo, found[0].id, "confirmed");
    let asked = verdict_of(&demo, &fake, &side, std::slice::from_ref(&b));
    assert!(matches!(asked.0, Verdict::Takes(_)), "{asked:?}");
    let both = verdict_of(&demo, &fake, &side, &[a, b]);
    assert!(
        refused(&both).contains(&format!(
            "finding {} is confirmed and names no fix",
            found[0].id
        )),
        "{both:?}"
    );
}

#[test]
fn a_group_whose_files_a_later_commit_changed_back_answers_no_findings() {
    let demo = demo("landing-reverted");
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
    std::fs::create_dir_all(demo.main.join("src")).unwrap();
    demo.commit(&demo.main, "src/a.kt");
    let side = demo.add_side(1, "agent", "work");
    std::fs::write(side.join("src/a.kt"), "changed\n").unwrap();
    crate::testing::git(&side, &["add", "src/a.kt"]);
    let changed = commit_with(&side, "a.rs", "Change src/a.kt");
    let changed_back = {
        std::fs::write(side.join("src/a.kt"), "src/a.kt").unwrap();
        crate::testing::git(&side, &["commit", "-q", "-a", "-m", "Change src/a.kt back"]);
        head(&side)
    };
    let reported = review_of(&demo, &fake, &side);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{}", reported.text);
    assert_eq!(prompts[0].0, "alpha");
    assert!(
        reported.text.contains("gamma (sonnet): no findings")
            && reported
                .text
                .contains("gamma: the diff holds none of the files its rules cover"),
        "{}",
        reported.text
    );
    assert!(reported.refusal.is_none(), "{:?}", reported.refusal);
    let asked = verdict_of(&demo, &fake, &side, &[changed, changed_back]);
    assert!(matches!(asked.0, Verdict::Takes(_)), "{asked:?}");
}
