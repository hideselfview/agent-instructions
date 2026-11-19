use super::*;
use crate::lists::Stamp;
use crate::lists::fixtures::NOON;
use crate::lists::item_fixtures::{Demo, demo};
use crate::review::fakes::fake;

use crate::side_git;

/// What git prints in <side> for <args>, trimmed.
fn git(side: &Path, args: &[&str]) -> String {
    boards::git_text(boards::git(side).args(args), "ask git")
        .unwrap()
        .trim()
        .to_string()
}

/// The commits of <side> past main, newest first, as git reads them for a card.
fn ahead(side: &Path) -> Vec<side_git::Commit> {
    git(side, &["rev-list", "main..HEAD"])
        .lines()
        .map(|full| side_git::Commit {
            sha: full.chars().take(7).collect(),
            full: full.to_string(),
            time: 1,
            authored: 1,
            subject: "s".to_string(),
        })
        .collect()
}

/// What the daemon reads of <side>'s git for its card, with <ahead> as the
/// commits waiting to land.
fn read_git(side: &Path, ahead: Vec<side_git::Commit>) -> SideRead {
    SideRead {
        head: ahead.first().cloned(),
        ahead,
        main: git(side, &["rev-parse", "origin/main"]),
        uncommitted: 0,
        last_write: None,
    }
}

/// The standing of <side> on <branch>, whose commits waiting to land git
/// reads as <git>, with <demo>'s store at <version>.
fn standing_at(
    reads: &Reads,
    demo: &Demo,
    side: &Path,
    branch: &str,
    git: &SideRead,
    version: u64,
) -> Result<Option<ItemWait>, String> {
    let worktree = Worktree {
        repo: "demo",
        top: side,
        branch: Some(branch),
        git,
    };
    reads.standing(
        &demo.store,
        version,
        &demo.tool.places.claude_sessions(),
        &worktree,
    )
}

/// The standing of <side> as the daemon reads it now.
fn standing(reads: &Reads, demo: &Demo, side: &Path) -> Option<ItemWait> {
    standing_at(
        reads,
        demo,
        side,
        &git(side, &["branch", "--show-current"]),
        &read_git(side, ahead(side)),
        demo.store.revision().unwrap(),
    )
    .unwrap()
}

fn review(demo: &Demo, root: &Path, program: &Path, side: &Path, key: &str) {
    crate::review::review(&crate::review::Inputs {
        store: &demo.store,
        project: "demo",
        repo: "demo",
        worktree: side,
        base: "main",
        head: None,
        key,
        root,
        reviewer: program,
        stamp: &Stamp {
            at: NOON,
            by: "tester".to_string(),
        },
        brief: None,
        runner: None,
    })
    .unwrap();
}

#[test]
fn a_card_follows_the_review_from_none_to_findings_to_fixes_and_says_nothing_once_land_takes_it() {
    let demo = demo("review-cards");
    let fake = fake(&demo.scratch.dir);
    let reads = Reads::with_root(Ok(fake.root.clone()));
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "notes.txt");
    assert_eq!(
        standing(&reads, &demo, &side),
        None,
        "no rule covers the commits"
    );
    demo.commit(&side, "a.rs");
    assert_eq!(
        standing(&reads, &demo, &side),
        Some(ItemWait::RulesReview(ReviewWait::NotRun))
    );
    // A review of the newest commit holds its file locked while it runs.
    let newest = ahead(&side)[0].full.clone();
    let path = crate::review::running_file(&demo.store.dir, &newest);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let held = crate::queue::open_locked(&path).unwrap();
    assert_eq!(
        standing(&reads, &demo, &side),
        Some(ItemWait::RulesReview(ReviewWait::Running))
    );
    unsafe { libc::close(held) };
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some(("Never do the bad thing here.", "a.rs", 1, "a.rs", "p")),
        )],
    );
    review(&demo, &fake.root, &fake.program, &side, "branch:work");
    assert_eq!(
        standing(&reads, &demo, &side),
        Some(ItemWait::RulesReview(ReviewWait::ToSettle(
            1,
            Settler::Coordinator
        )))
    );
    // The store's first finding.
    let settle: Vec<String> = [
        "settle",
        "1",
        "confirmed",
        "--why",
        "real",
        "--project",
        "demo",
        "--by",
        "tester",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    crate::review::cmd_review(&demo.tool, &settle).unwrap();
    assert_eq!(
        standing(&reads, &demo, &side),
        Some(ItemWait::RulesReview(ReviewWait::Open(1)))
    );
    // A branch whose review found nothing is ready to land.
    let clean = demo.add_side(2, "agent", "clean");
    demo.commit(&clean, "b.rs");
    fake.answer("alpha", &[("rule-a", None)]);
    review(&demo, &fake.root, &fake.program, &clean, "branch:clean");
    assert_eq!(standing(&reads, &demo, &clean), None);
    // A commit added after its review changes the branch's net change in one
    // file, which a new run of the review reads.
    demo.commit(&clean, "c.rs");
    assert_eq!(
        standing(&reads, &demo, &clean),
        Some(ItemWait::RulesReview(ReviewWait::Unreviewed(1)))
    );
}

#[test]
fn a_card_whose_project_has_no_store_says_so_and_never_that_land_takes_its_commits() {
    let demo = demo("review-cards-no-store");
    let fake = fake(&demo.scratch.dir);
    let reads = Reads::with_root(Ok(fake.root.clone()));
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let ghost = Store::named(&demo.tool.places, "ghost").unwrap();
    let git = read_git(&side, ahead(&side));
    let worktree = Worktree {
        repo: "demo",
        top: &side,
        branch: Some("work"),
        git: &git,
    };
    assert_eq!(
        reads.standing(&ghost, 1, &demo.tool.places.claude_sessions(), &worktree),
        Err("there is no store for project ghost".to_string())
    );
}

#[test]
fn a_card_whose_commits_git_cannot_read_says_why_and_asks_git_again_next_time() {
    let demo = demo("review-cards-unread");
    let fake = fake(&demo.scratch.dir);
    let reads = Reads::with_root(Ok(fake.root.clone()));
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let mut missing = ahead(&side);
    missing[0].full = "0".repeat(40);
    let git = read_git(&side, missing);
    let version = demo.store.revision().unwrap();
    for _ in 0..2 {
        let said = standing_at(&reads, &demo, &side, "work", &git, version).unwrap_err();
        assert!(said.contains("reading the paths the commits of"), "{said}");
    }
    assert!(
        reads.commits.lock().unwrap().is_empty(),
        "Reads keeps no commits git could not read"
    );
    assert!(
        reads.judged.lock().unwrap().is_empty(),
        "Reads keeps no judgment of commits git could not read"
    );
}

#[test]
fn findings_to_settle_wait_on_the_judge_while_one_listens() {
    let demo = demo("review-cards-judge");
    let fake = fake(&demo.scratch.dir);
    let reads = Reads::with_root(Ok(fake.root.clone()));
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    fake.answer(
        "alpha",
        &[(
            "rule-a",
            Some(("Never do the bad thing here.", "a.rs", 1, "a.rs", "p")),
        )],
    );
    review(&demo, &fake.root, &fake.program, &side, "branch:work");
    let judge =
        crate::inbox::Messaging::new(demo.scratch.dir.join("j.sock").to_str().unwrap(), None)
            .unwrap();
    let stamp = Stamp {
        at: NOON,
        by: "judge".to_string(),
    };
    let session = crate::ack::agent::Caller {
        session: "s-judge".to_string(),
        id: "s-judge".to_string(),
    };
    demo.store
        .write(&stamp, "judge listen", |db| {
            crate::review::judge::listen(db, &judge, &session, &stamp)
        })
        .unwrap();
    crate::testing::running_session(&demo.tool.places, "s-judge");
    assert_eq!(
        standing(&reads, &demo, &side),
        Some(ItemWait::RulesReview(ReviewWait::ToSettle(
            1,
            Settler::Judge
        )))
    );
}

#[test]
fn a_worktree_is_judged_once_per_tip_and_again_when_its_branch_main_or_store_moves() {
    let demo = demo("review-cards-per-tip");
    let fake = fake(&demo.scratch.dir);
    let reads = Reads::with_root(Ok(fake.root.clone()));
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let git = read_git(&side, ahead(&side));
    let version = demo.store.revision().unwrap();
    let not_run = Some(ItemWait::RulesReview(ReviewWait::NotRun));
    assert_eq!(
        standing_at(&reads, &demo, &side, "work", &git, version),
        Ok(not_run.clone())
    );
    // With its git folder gone, git can read nothing in the worktree, so
    // only a judgment kept from the first build answers.
    let dot_git = side.join(".git");
    let away = demo.scratch.dir.join("dot-git");
    std::fs::rename(&dot_git, &away).unwrap();
    assert_eq!(
        standing_at(&reads, &demo, &side, "work", &git, version),
        Ok(not_run.clone()),
        "the same tip is judged once"
    );
    let mut moved_main = git.clone();
    moved_main.main = "1".repeat(40);
    let tips = [
        ("another branch", "other", &git, version),
        ("a moved origin/main", "work", &moved_main, version),
        ("a write to the store", "work", &git, version + 1),
    ];
    for (what, branch, git, version) in tips {
        let said = standing_at(&reads, &demo, &side, branch, git, version);
        assert!(said.is_err(), "{what} is judged again, from git: {said:?}");
    }
    std::fs::rename(&away, &dot_git).unwrap();
}

#[test]
fn a_card_whose_rules_cannot_be_read_says_why_and_never_that_land_takes_its_commits() {
    let demo = demo("review-cards-no-rules");
    let reads = Reads::with_root(Ok(demo.scratch.dir.join("no-rules")));
    let side = demo.add_side(1, "agent", "work");
    demo.commit(&side, "a.rs");
    let said = standing_at(
        &reads,
        &demo,
        &side,
        "work",
        &read_git(&side, ahead(&side)),
        demo.store.revision().unwrap(),
    )
    .unwrap_err();
    assert!(
        said.starts_with("reading the rules of demo from "),
        "{said}"
    );
}
