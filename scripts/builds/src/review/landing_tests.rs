use super::*;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::{
    Fake, Raised, commit_text_group, fake, group_file, style_group, style_site, ten_lines,
};
use crate::review::files;
use crate::review::{Inputs, review};

/// The change on branch `work` of the repo `demo`.
const WORK: Change = Change {
    repo: "demo",
    key: "branch:work",
};

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
        runner: None,
    })
    .unwrap()
}

/// What `builds land` would ask of <shas> in <side>: the verdict and its
/// words.
fn verdict_of(demo: &Demo, fake: &Fake, side: &Path, shas: &[String]) -> (Verdict, Option<String>) {
    verdict_checked(demo, fake, side, shas, CodeCheck::Branch("HEAD"))
}

/// `verdict_of`, reading the branch as <check> says.
fn verdict_checked(
    demo: &Demo,
    fake: &Fake,
    side: &Path,
    shas: &[String],
    check: CodeCheck,
) -> (Verdict, Option<String>) {
    let asked = asked(demo, fake, side, shas);
    let verdict = judge(
        &asked.conn,
        asked.change(),
        side,
        &asked.needs,
        check,
        asked.scope(),
    )
    .unwrap();
    let words = refusal(&asked.needs, &verdict);
    (verdict, words)
}

fn asked(demo: &Demo, fake: &Fake, side: &Path, shas: &[String]) -> Asked {
    asked_as(demo, fake, side, shas, "branch:work")
}

/// What `builds land` asks of <shas> in <side> for the change <key>.
fn asked_as(demo: &Demo, fake: &Fake, side: &Path, shas: &[String], key: &str) -> Asked {
    let mut words: Vec<String> = vec![side.display().to_string()];
    words.extend(shas.iter().cloned());
    words.extend([
        "--project".to_string(),
        "demo".to_string(),
        "--landing".to_string(),
        key.to_string(),
        "--rules-root".to_string(),
        fake.root.display().to_string(),
    ]);
    let args = Args::parse(&words, &["project", "landing", "rules-root"], &[]).unwrap();
    ask_of(&demo.tool, &args)
}

fn ask_of(tool: &Tool, args: &Args) -> Asked {
    super::ask(tool, args).unwrap()
}

fn findings(demo: &Demo) -> Vec<record::Finding> {
    let conn = demo.store.open_existing().unwrap();
    let landing = record::open_landing(&conn, WORK).unwrap().unwrap();
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
        words.contains("no review of this change is open") && words.contains(ONE_REVIEW),
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
fn a_fix_commit_is_read_by_a_new_run_once_which_closes_the_finding_it_fixes() {
    let demo = demo("landing-fix");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    fake.answer("alpha", &[("rule-a", Some(BAD))]);
    review_of(&demo, &fake, &side);
    let finding = findings(&demo)[0].id;
    settle(&demo, finding, "confirmed");
    let asked = verdict_of(&demo, &fake, &side, std::slice::from_ref(&a));
    let Verdict::Unfinished(_, unfinished) = &asked.0 else {
        panic!("{asked:?}")
    };
    assert_eq!(unfinished.open[0].id, finding);
    assert!(
        refused(&asked).contains("1 confirmed finding is open"),
        "{asked:?}"
    );
    let b = demo.commit(&side, "b.rs");
    let asked = verdict_of(&demo, &fake, &side, &[a.clone(), b.clone()]);
    assert!(
        refused(&asked).contains("the branch changed since the last review, run"),
        "{asked:?}"
    );
    fake.prompts();
    fake.closes(&[finding]);
    let reported = review_of(&demo, &fake, &side);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "{}", reported.text);
    assert!(
        prompts[0].1.contains("+b.rs") && !prompts[0].1.contains("+a.rs"),
        "{}",
        prompts[0].1
    );
    assert!(reported.refusal.is_none(), "{:?}", reported.refusal);
    let again = review(&Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: &side,
        base: "main",
        head: None,
        key: "branch:work",
        root: &fake.root,
        reviewer: &fake.program,
        stamp: &stamp(),
        brief: None,
        runner: None,
    })
    .unwrap_err()
    .text();
    assert!(again.contains(ONE_REVIEW), "{again}");
    assert!(fake.prompts().is_empty());
    let asked = verdict_of(&demo, &fake, &side, &[a, b]);
    assert!(matches!(asked.0, Verdict::Takes(_)), "{asked:?}");
}

#[test]
fn a_rebase_reads_nothing_and_leaves_an_open_finding_open() {
    let demo = demo("landing-rebase-open");
    let fake = fake(&demo.scratch.dir);
    commit_text(&demo.main, "f.rs", &ten_lines(&[]), "Add f");
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    let side = demo.add_side(1, "agent", "work");
    commit_text(&side, "f.rs", &ten_lines(&[(2, "two-branch")]), "Change f");
    alpha_finds(&fake, &[("f.rs", "two-branch")]);
    review_of(&demo, &fake, &side);
    let finding = findings(&demo)[0].id;
    settle(&demo, finding, "confirmed");
    commit_text(
        &demo.main,
        "f.rs",
        &ten_lines(&[(9, "nine-main")]),
        "Change f on main",
    );
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    crate::testing::git(&side, &["rebase", "-q", "origin/main"]);
    let rebased = head(&side);
    fake.prompts();
    let asked = verdict_of(&demo, &fake, &side, std::slice::from_ref(&rebased));
    let Verdict::Unfinished(_, unfinished) = &asked.0 else {
        panic!("{asked:?}")
    };
    assert!(unfinished.unreviewed.is_empty(), "{asked:?}");
    assert_eq!(unfinished.open[0].id, finding);
    assert!(!check_landing(&demo, &fake, &side, &[rebased]));
    assert!(fake.prompts().is_empty(), "nothing read the rebased branch");
    assert!(findings(&demo)[0].is_open());
}

#[test]
fn a_commit_added_after_the_review_is_read_as_the_difference_and_the_branch_lands_in_parts() {
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
    assert_eq!(unfinished.unreviewed, ["c.rs"]);
    let words = refused(&said);
    assert!(
        words.contains(ONE_REVIEW) && words.contains("Run builds review in the branch's worktree"),
        "{words}"
    );
    // land reads the branch's net change, so a part waits too.
    assert!(matches!(
        verdict_of(&demo, &fake, &side, std::slice::from_ref(&a)).0,
        Verdict::Unfinished(..)
    ));
    fake.prompts();
    review_of(&demo, &fake, &side);
    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].1.contains("+c.rs") && !prompts[0].1.contains("+a.rs"));
    let part = asked(&demo, &fake, &side, &[a, b]);
    assert_eq!(record_landed(&part, &stamp(), true).unwrap(), None);
    assert!(matches!(
        verdict_of(&demo, &fake, &side, std::slice::from_ref(&c)).0,
        Verdict::Takes(_)
    ));
    let rest = asked(&demo, &fake, &side, &[c]);
    assert!(record_landed(&rest, &stamp(), false).unwrap().is_some());
}

#[test]
fn land_refuses_a_branch_whose_net_change_no_run_read() {
    let demo = demo("landing-unread");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    demo.commit(&side, "b.rs");
    review_of(&demo, &fake, &side);
    std::fs::write(side.join("b.rs"), "resolved\n").unwrap();
    crate::testing::git(&side, &["commit", "-q", "--amend", "-a", "--no-edit"]);
    let changed = head(&side);
    let asked = verdict_of(&demo, &fake, &side, &[a, changed]);
    let Verdict::Unfinished(covered, unfinished) = &asked.0 else {
        panic!("{asked:?}")
    };
    assert_eq!(unfinished.unreviewed, ["b.rs"]);
    assert!(
        refused(&asked).contains(&format!(
            "the branch changed since the last review, run {}, in b.rs",
            covered.run
        )),
        "{asked:?}"
    );
}

#[test]
fn land_takes_a_rebased_branch_whose_net_change_runs_read() {
    let demo = demo("landing-rebase");
    let fake = fake(&demo.scratch.dir);
    commit_text(&demo.main, "f.rs", &ten_lines(&[]), "Add f");
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    let side = demo.add_side(1, "agent", "work");
    commit_text(&side, "f.rs", &ten_lines(&[(2, "two-branch")]), "Change f");
    review_of(&demo, &fake, &side);
    commit_text(
        &demo.main,
        "f.rs",
        &ten_lines(&[(9, "nine-main")]),
        "Change f on main",
    );
    demo.commit(&demo.main, "m.rs");
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
    crate::testing::git(&side, &["rebase", "-q", "origin/main"]);
    let rebased = head(&side);
    fake.prompts();
    let asked = verdict_of(&demo, &fake, &side, std::slice::from_ref(&rebased));
    assert!(matches!(asked.0, Verdict::Takes(_)), "{asked:?}");
    assert!(check_landing(&demo, &fake, &side, &[rebased]));
    assert!(fake.prompts().is_empty(), "nothing read the rebased branch");
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
    let asked_picked = asked_as(
        &demo,
        &fake,
        &other,
        std::slice::from_ref(&picked),
        "branch:work",
    );
    let conn = demo.store.open_existing().unwrap();
    assert!(matches!(
        judge(
            &conn,
            WORK,
            &other,
            &asked_picked.needs,
            CodeCheck::Branch("HEAD"),
            asked_picked.scope(),
        )
        .unwrap(),
        Verdict::Takes(_)
    ));
    drop(conn);
    let landing = record_landed(&asked_picked, &stamp(), false).unwrap();
    assert!(landing.is_some());
    // Recording again finds no open landing and records nothing more.
    assert_eq!(record_landed(&asked_picked, &stamp(), false).unwrap(), None);
    let conn = demo.store.open_existing().unwrap();
    assert!(
        record::landing(&conn, landing.unwrap())
            .unwrap()
            .landed_run
            .is_some()
    );
    assert_eq!(record::open_landing(&conn, WORK).unwrap(), None);
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
        "medium",
        "enumerate = true\nsites = \"handles rows\"\n",
    );
    fake.answer_task("gamma", serde_json::json!({"summary": "s", "sites": []}));
    std::fs::create_dir_all(demo.main.join("src")).unwrap();
    demo.commit(&demo.main, "src/a.kt");
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
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
fn a_branch_lands_in_two_parts_once_a_review_closed_its_findings() {
    let demo = demo("landing-parts");
    let fake = fake(&demo.scratch.dir);
    group_file(
        &fake.root,
        "alpha",
        "alpha",
        "medium",
        "commit_text = true\n",
    );
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
    std::fs::write(side.join("a.rs"), "fixed\n").unwrap();
    crate::testing::git(&side, &["commit", "-q", "-a", "-m", "Fix a"]);
    let fa = head(&side);
    // The review closes the finding on a.rs; the one on b's message stays open,
    // and keeps every part of the branch from landing.
    fake.answer_task(
        "alpha",
        serde_json::json!({
            "rules": [{"rule": "rule-a", "verdict": "none", "findings": []}],
            "follow_up": [
                {"id": found[0].id, "verdict": "fixed"},
                {"id": found[1].id, "verdict": "open", "reason": "The commit message still has a vague line."},
            ],
        }),
    );
    review_of(&demo, &fake, &side);
    let part = verdict_of(&demo, &fake, &side, &[a.clone(), fa.clone()]);
    assert!(
        refused(&part).contains("1 confirmed finding is open"),
        "{part:?}"
    );
    let stamp = stamp();
    demo.store
        .write(&stamp, "settle again", |db| {
            record::settle_again(db, found[1].id, "the line says enough", stamp.at, "tester")
        })
        .unwrap();
    let first = asked(&demo, &fake, &side, &[a, fa]);
    assert_eq!(
        record_landed(&first, &stamp, true).unwrap(),
        None,
        "b waits to land, so the landing stays open"
    );
    let second = verdict_of(&demo, &fake, &side, std::slice::from_ref(&b));
    assert!(matches!(second.0, Verdict::Takes(_)), "{second:?}");
    let second = asked(&demo, &fake, &side, &[b]);
    assert!(record_landed(&second, &stamp, false).unwrap().is_some());
}

/// The commit HEAD names in <side>.
fn head(side: &Path) -> String {
    boards::git_text(boards::git(side).args(["rev-parse", "HEAD"]), "read HEAD")
        .unwrap()
        .trim()
        .to_string()
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
        "medium",
        "enumerate = true\nsites = \"handles rows\"\n",
    );
    std::fs::create_dir_all(demo.main.join("src")).unwrap();
    demo.commit(&demo.main, "src/a.kt");
    crate::testing::git(&demo.main, &["push", "-q", "origin", "main"]);
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

/// Writes <text> to <file> in <top>, commits it with <message>, and returns
/// the commit.
fn commit_text(top: &Path, file: &str, text: &str, message: &str) -> String {
    std::fs::write(top.join(file), text).unwrap();
    crate::testing::git(top, &["add", file]);
    crate::testing::git(top, &["commit", "-q", "-m", message]);
    head(top)
}

/// Runs `builds review check-landing` on <shas> in <side> as `tester`, and
/// says whether it takes them.
fn check_landing(demo: &Demo, fake: &Fake, side: &Path, shas: &[String]) -> bool {
    let mut words: Vec<String> = vec![side.display().to_string()];
    words.extend(shas.iter().cloned());
    words.extend(
        [
            "--project",
            "demo",
            "--landing",
            "branch:work",
            "--rules-root",
        ]
        .iter()
        .map(|w| w.to_string()),
    );
    words.push(fake.root.display().to_string());
    match check_landing_command(&demo.tool, &words) {
        Ok(()) => true,
        Err(Fail::Exit(1)) => false,
        Err(other) => panic!("check-landing failed: {}", other.text()),
    }
}

/// Makes a branch whose second commit adds `gallery_count` to a.rs, reviews
/// it, confirms the finding on that line, and returns the demo, its reviewer,
/// the side worktree, both commits and the finding.
fn confirmed_on_the_second_commit(name: &str) -> (Demo, Fake, PathBuf, String, String, i64) {
    let demo = demo(name);
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let first = commit_text(&side, "a.rs", "keep\n", "Add a");
    let second = commit_text(&side, "a.rs", "keep\ngallery_count\n", "Count the gallery");
    alpha_finds(&fake, &[("a.rs", "gallery_count")]);
    review_of(&demo, &fake, &side);
    let found = findings(&demo);
    assert_eq!(found.len(), 1, "{found:?}");
    settle(&demo, found[0].id, "confirmed");
    (demo, fake, side, first, second, found[0].id)
}

#[test]
fn land_refuses_a_reworded_message_until_a_review_reads_it() {
    let demo = demo("landing-reworded");
    let fake = fake(&demo.scratch.dir);
    commit_text_group(&fake);
    let side = demo.add_side(1, "agent", "work");
    commit_with(&side, "a.rs", "Add a");
    review_of(&demo, &fake, &side);
    crate::testing::git(&side, &["commit", "-q", "--amend", "-m", "Add the file a"]);
    let reworded = [head(&side)];
    let said = verdict_of(&demo, &fake, &side, &reworded);
    let Verdict::Unfinished(_, unfinished) = &said.0 else {
        panic!("{said:?}")
    };
    assert_eq!(unfinished.unreviewed, [files::COMMIT_MESSAGE], "{said:?}");
    review_of(&demo, &fake, &side);
    let said = verdict_of(&demo, &fake, &side, &reworded);
    assert!(matches!(said.0, Verdict::Takes(_)), "{said:?}");
}

/// Group style judges no commit message, and group alpha reads them: builds
/// drops the style group's finding on a message, and a reworded message
/// waits only for alpha's answer.
#[test]
fn a_reworded_message_raises_no_style_finding_and_lands_once_alpha_answers() {
    let demo = demo("landing-message-style");
    let fake = fake(&demo.scratch.dir);
    group_file(
        &fake.root,
        "alpha",
        "alpha",
        "medium",
        "commit_text = true\n",
    );
    style_group(
        &fake,
        serde_json::json!([style_site(files::COMMIT_MESSAGE, "a vague line")]),
    );
    let side = demo.add_side(1, "agent", "work");
    commit_with(&side, "a.rs", "Add a with a vague line");
    review_of(&demo, &fake, &side);
    let found = findings(&demo);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        found[0].check_failed.as_deref(),
        Some("it is on the commit messages (COMMIT_MESSAGE), which group style does not read")
    );
    fake.prompts();
    crate::testing::git(&side, &["commit", "-q", "--amend", "-m", "Add the file a"]);
    let reworded = [head(&side)];
    let said = verdict_of(&demo, &fake, &side, &reworded);
    let Verdict::Unfinished(_, unfinished) = &said.0 else {
        panic!("{said:?}")
    };
    assert_eq!(unfinished.unreviewed, [files::COMMIT_MESSAGE], "{said:?}");
    review_of(&demo, &fake, &side);
    let prompts = fake.prompts();
    let groups: Vec<&str> = prompts.iter().map(|(group, _)| group.as_str()).collect();
    assert_eq!(groups, ["alpha"]);
    assert!(
        prompts[0].1.contains("diff --git a/COMMIT_MESSAGE")
            && prompts[0].1.contains("+Add the file a"),
        "{}",
        prompts[0].1
    );
    let said = verdict_of(&demo, &fake, &side, &reworded);
    assert!(matches!(said.0, Verdict::Takes(_)), "{said:?}");
}

#[test]
fn land_refuses_while_a_confirmed_finding_is_open() {
    let (demo, fake, side, first, second, finding) = confirmed_on_the_second_commit("landing-open");
    let shas = [first, second];
    assert!(!check_landing(&demo, &fake, &side, &shas));
    let said = verdict_of(&demo, &fake, &side, &shas);
    let Verdict::Unfinished(_, unfinished) = &said.0 else {
        panic!("{said:?}")
    };
    assert_eq!(unfinished.open.len(), 1);
    assert_eq!(unfinished.open[0].id, finding);
    assert!(unfinished.unreviewed.is_empty());
    assert!(
        refused(&said).contains("1 confirmed finding is open"),
        "{said:?}"
    );
}

#[test]
fn a_review_of_a_dropped_commit_closes_its_finding_and_the_branch_lands() {
    let (demo, fake, side, first, _, finding) = confirmed_on_the_second_commit("landing-dropped");
    // The agent drops the commit that added the code.
    crate::testing::git(&side, &["reset", "-q", "--hard", &first]);
    let shas = std::slice::from_ref(&first);
    let said = verdict_of(&demo, &fake, &side, shas);
    let Verdict::Unfinished(_, unfinished) = &said.0 else {
        panic!("{said:?}")
    };
    assert_eq!(unfinished.unreviewed, ["a.rs"]);
    assert_eq!(unfinished.open[0].id, finding);
    fake.closes(&[finding]);
    review_of(&demo, &fake, &side);
    assert!(check_landing(&demo, &fake, &side, shas));
    let shown = render::finding_text(&findings(&demo)[0]);
    assert!(shown.contains("closed the finding"), "{shown}");
    let asked = asked(&demo, &fake, &side, shas);
    assert!(record_landed(&asked, &stamp(), false).unwrap().is_some());
}

#[test]
fn a_confirmed_finding_settled_again_as_dropped_blocks_no_landing() {
    let (demo, fake, side, first, second, finding) =
        confirmed_on_the_second_commit("landing-settle-again");
    let shas = [first, second];
    assert!(!check_landing(&demo, &fake, &side, &shas));
    let stamp = stamp();
    demo.store
        .write(&stamp, "settle again", |db| {
            record::settle_again(db, finding, "the count is used", stamp.at, "tester")
        })
        .unwrap();
    assert!(check_landing(&demo, &fake, &side, &shas));
    let shown = render::finding_text(&findings(&demo)[0]);
    assert!(
        shown.contains("settled confirmed by tester: judged")
            && shown.contains("settled again as dropped by tester: the count is used"),
        "{shown}"
    );
}

#[test]
fn a_confirmed_finding_the_person_allowed_blocks_no_landing() {
    let (demo, fake, side, first, second, finding) =
        confirmed_on_the_second_commit("landing-allowed");
    let shas = [first, second];
    assert!(!check_landing(&demo, &fake, &side, &shas));
    let stamp = stamp();
    demo.store
        .write(&stamp, "allow", |db| {
            record::allow(
                db,
                finding,
                "the person: land it as it is",
                stamp.at,
                "tester",
            )
        })
        .unwrap();
    assert!(check_landing(&demo, &fake, &side, &shas));
    let shown = render::finding_text(&findings(&demo)[0]);
    assert!(
        shown.contains("settled confirmed by tester: judged")
            && shown.contains(
                "the person allowed it to land, and tester recorded their words: the person: land it as it is"
            ),
        "{shown}"
    );
}

/// What `builds health` says of the branch in <side>, whose commits are
/// <shas>, as the clauses of `blockers` and as the advice `idle::landing` gives.
fn idle_says(
    demo: &Demo,
    fake: &Fake,
    side: &Path,
    shas: &[String],
) -> (Vec<String>, crate::health::Advice) {
    let (verdict, _) = verdict_of(demo, fake, side, shas);
    let lists = demo.store.load().unwrap();
    let advice = crate::review::idle::landing(&verdict, &lists, side, shas).advice();
    (blockers(&verdict), advice)
}

#[test]
fn an_idle_branch_with_an_open_finding_names_the_finding_and_who_acts() {
    let (demo, fake, side, first, second, finding) = confirmed_on_the_second_commit("idle-open");
    let shas = [first, second];
    let (blocked, advice) = idle_says(&demo, &fake, &side, &shas);
    assert_eq!(
        blocked,
        [format!(
            "finding {finding} is confirmed and open: its agent must fix it, or the person must allow it"
        )]
    );
    assert!(advice.command.is_none(), "{advice:?}");
    assert!(advice.text.contains(&blocked[0]), "{advice:?}");
}

#[test]
fn an_idle_branch_with_an_unsettled_finding_waits_for_the_judge() {
    let demo = demo("idle-unsettled");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = commit_text(&side, "a.rs", "gallery_count\n", "Add a");
    alpha_finds(&fake, &[("a.rs", "gallery_count")]);
    review_of(&demo, &fake, &side);
    let finding = findings(&demo)[0].id;
    let (blocked, _) = idle_says(&demo, &fake, &side, &[a]);
    assert_eq!(
        blocked,
        [format!(
            "finding {finding} waits for the judge to settle it (the coordinator when no judge listens)"
        )]
    );
}

#[test]
fn an_idle_branch_changed_since_its_review_names_the_unread_paths() {
    let (demo, fake, side, first, second, _) = confirmed_on_the_second_commit("idle-unread");
    let third = commit_text(&side, "a.rs", "keep\ngallery_count\nmore\n", "Add more");
    let (blocked, _) = idle_says(&demo, &fake, &side, &[first, second, third]);
    assert!(
        blocked
            .iter()
            .any(|clause| clause.starts_with("no review has read the changes to a.rs")),
        "{blocked:?}"
    );
}

#[test]
fn an_idle_branch_with_review_clear_still_mentions_app_coverage() {
    let (demo, fake, side, first, second, finding) =
        confirmed_on_the_second_commit("idle-settled-dropped");
    let stamp = stamp();
    demo.store
        .write(&stamp, "settle again", |db| {
            record::settle_again(db, finding, "the count is used", stamp.at, "tester")
        })
        .unwrap();
    let shas = [first, second.clone()];
    let (blocked, advice) = idle_says(&demo, &fake, &side, &shas);
    assert!(blocked.is_empty(), "{blocked:?}");
    assert_eq!(
        advice.text,
        "the review check allows these commits. To check app coverage and land, run"
    );
    assert_eq!(
        advice.command.as_deref(),
        Some(format!("builds land {} {} {second}", side.display(), shas[0]).as_str())
    );
}

#[test]
fn an_idle_branch_with_no_review_open_says_its_agent_must_review() {
    let demo = demo("idle-no-run");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let a = demo.commit(&side, "a.rs");
    let (blocked, _) = idle_says(&demo, &fake, &side, &[a]);
    assert_eq!(
        blocked,
        ["no review of the branch is open: its agent must run builds review"]
    );
}

#[test]
fn a_landing_of_design_files_only_starts_no_audit() {
    let demo = demo("landing-design-only");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let stamp = stamp();
    demo.store
        .write(&stamp, "design-globs", |db| {
            crate::lists::project::set_design_globs(
                db,
                crate::lists::project::GlobKind::Place,
                &[("demo".to_string(), vec!["design/*.rs".to_string()])].into(),
            )
        })
        .unwrap();
    std::fs::create_dir_all(side.join("design")).unwrap();
    let board = demo.commit(&side, "design/board.html");
    let code = demo.commit(&side, "a.rs");

    let design = asked(&demo, &fake, &side, std::slice::from_ref(&board));
    assert!(holds_design_only(&design).unwrap());
    // The sample picks every landing at roll 0, and this one starts none.
    assert!(start_audit(&design, 1, 0.0).is_ok());

    let mixed = asked(&demo, &fake, &side, &[board, code]);
    assert!(!holds_design_only(&mixed).unwrap());
    // Past the design-only check, the audit asks the store about landing 1,
    // which has no record: the design landing never got that far.
    assert!(start_audit(&mixed, 1, 0.0).is_err());
}

/// <body> repeated past the review cap, committed as <file> in <side>.
fn big_commit(side: &Path, file: &str, body: &str) -> String {
    let path = side.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let lines = check::MAX_ADDED_LINES as usize + 1;
    std::fs::write(&path, body.repeat(lines)).unwrap();
    crate::testing::git(side, &["add", file]);
    crate::testing::git(side, &["commit", "-q", "-m", file]);
    crate::testing::git_out(side, &["rev-parse", "HEAD"])
}

fn waits_over_cap_of(demo: &Demo, fake: &Fake, side: &Path, sha: &str) -> Option<String> {
    let asked = asked(demo, fake, side, &[sha.to_string()]);
    super::waits_over_cap(&asked).unwrap()
}

#[test]
fn a_branch_over_the_cap_that_needs_no_review_lands_with_land_saying_why() {
    let demo = demo("landing-over-cap-design");
    let fake = fake(&demo.scratch.dir);
    // A rule that judges design files covers the board, so the branch needs
    // a review.
    crate::review::fakes::looks_group(&fake);
    let side = demo.add_side(1, "agent", "work");
    let stamp = stamp();
    demo.store
        .write(&stamp, "design-globs", |db| {
            crate::lists::project::set_design_globs(
                db,
                crate::lists::project::GlobKind::Place,
                &[("demo".to_string(), vec!["design/*.rs".to_string()])].into(),
            )
        })
        .unwrap();
    let board = big_commit(&side, "design/board.rs", "let x = 1;\n");
    let reported = review_of(&demo, &fake, &side);
    assert!(reported.refusal.is_none(), "{}", reported.text);
    assert!(fake.prompts().is_empty());
    // The review's verdict takes the branch, so land, the dashboard's card
    // and health all say it lands.
    let (verdict, refusal) = verdict_of(&demo, &fake, &side, std::slice::from_ref(&board));
    assert!(matches!(verdict, Verdict::OverCap(_)), "{verdict:?}");
    assert_eq!(refusal, None);
    assert!(blockers(&verdict).is_empty());
    let said = taken(&verdict);
    assert!(said.starts_with(over_cap::TAKEN), "{said}");
    assert!(said.contains("design files only"), "{said}");
    assert!(said.contains("patch id"), "{said}");
    // A rebase that keeps the lines keeps the answer: another commit that
    // changes a line makes it another branch.
    let code = big_commit(&side, "a.rs", "let x = 1;\n");
    let (verdict, _) = verdict_of(&demo, &fake, &side, &[board, code]);
    assert_eq!(verdict, Verdict::NoRun);
}

#[test]
fn a_branch_over_the_cap_that_waits_on_the_person_does_not_land() {
    let demo = demo("landing-over-cap-asked");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    let code = big_commit(&side, "a.rs", "let x = 1;\n");
    let reported = review_of(&demo, &fake, &side);
    assert!(reported.refusal.is_some());
    let Some(said) = waits_over_cap_of(&demo, &fake, &side, &code) else {
        panic!("a branch of code over the cap waits on the person")
    };
    assert!(said.contains("Question"), "{said}");
}

#[test]
fn a_branch_called_mechanical_lands_under_the_cap_with_land_saying_who_and_why() {
    let demo = demo("landing-over-cap-marked");
    let fake = fake(&demo.scratch.dir);
    let side = demo.add_side(1, "agent", "work");
    std::fs::write(side.join("a.rs"), "let x = 1;\n").unwrap();
    crate::testing::git(&side, &["add", "a.rs"]);
    crate::testing::git(&side, &["commit", "-q", "-m", "a"]);
    let code = crate::testing::git_out(&side, &["rev-parse", "HEAD"]);
    let key = crate::review::default_key(&side).unwrap();
    marked_mechanical::call_mechanical(
        &demo.store,
        "demo",
        &side,
        "origin/main",
        &key,
        ("agent-1", "the compiler"),
        &stamp(),
    )
    .unwrap();
    let (verdict, refusal) = verdict_of(&demo, &fake, &side, std::slice::from_ref(&code));
    assert!(matches!(verdict, Verdict::OverCap(_)), "{verdict:?}");
    assert_eq!(refusal, None);
    let said = taken(&verdict);
    assert!(
        said.contains("agent-1 called it mechanical, and the compiler covers it"),
        "{said}"
    );
}

/// The branch's two reviewed commits as one commit with a new message and
/// the same tree, as land's squash of a design change builds it.
fn squash_two(side: &Path, message: &str) -> String {
    crate::testing::git(side, &["reset", "-q", "--soft", "HEAD~2"]);
    crate::testing::git(side, &["commit", "-q", "-m", message]);
    head(side)
}

/// Whether land's check of a branch it squashed takes <shas>.
fn check_squashed(demo: &Demo, fake: &Fake, side: &Path, shas: &[String]) -> bool {
    verdict_checked(demo, fake, side, shas, CodeCheck::Squashed("HEAD"))
        .1
        .is_none()
}

/// A squash that leaves the net change as the review read it needs no new
/// review, though its message is new; a squash of changed files does.
#[test]
fn a_squash_with_the_reviewed_net_change_lands_without_a_new_review() {
    let demo = demo("landing-squashed");
    let fake = fake(&demo.scratch.dir);
    commit_text_group(&fake);
    let side = demo.add_side(1, "agent", "work");
    commit_text(&side, "a.rs", "keep\n", "Add a");
    commit_text(&side, "a.rs", "keep\nmore\n", "Add more");
    review_of(&demo, &fake, &side);
    let squashed = [squash_two(&side, "One change")];
    assert!(verdict_of(&demo, &fake, &side, &squashed).1.is_some());
    assert!(check_squashed(&demo, &fake, &side, &squashed));
    commit_text(&side, "a.rs", "keep\nmore\nand more\n", "Add and more");
    let changed = [head(&side)];
    assert!(!check_squashed(&demo, &fake, &side, &changed));
}
