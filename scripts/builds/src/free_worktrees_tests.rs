use super::*;
use crate::claim::cmd_release;
use crate::testing::{Scratch, git, repo};
use crate::worktree::cmd_new;

/// A repo `demo` pushed to a bare origin, and a tool on a scratch volume.
fn demo(scratch: &Scratch) -> (Tool, std::path::PathBuf) {
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    git(
        &scratch.dir,
        &["init", "-q", "--bare", &origin.display().to_string()],
    );
    git(
        &main,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&main, &["push", "-q", "origin", "main"]);
    (tool, main)
}

fn side(scratch: &Scratch, n: usize) -> std::path::PathBuf {
    scratch.dir.join(format!("demo-worktrees/side-{n}"))
}

fn release(scratch: &Scratch, n: usize) {
    cmd_release(&scratch.places(), &[side(scratch, n).display().to_string()]).unwrap();
}

fn new(tool: &Tool, main: &Path, extra: &[&str]) {
    let mut args = vec![main.display().to_string()];
    args.extend(extra.iter().map(|arg| arg.to_string()));
    cmd_new(tool, &crate::testing::no_ssh(), &args).unwrap();
}

/// A worktree of <main> that is no side worktree, recorded as its mac app
/// checkout.
fn app_checkout(tool: &Tool, scratch: &Scratch, main: &Path) -> std::path::PathBuf {
    let top = scratch.dir.join("demo-worktrees/mac-1");
    git(
        main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &top.display().to_string(),
            "origin/main",
        ],
    );
    crate::worktree::cmd_app_checkout(
        tool,
        &crate::testing::no_ssh(),
        &["mac".to_string(), top.display().to_string()],
    )
    .unwrap();
    assert!(
        top.join("target").is_symlink(),
        "app-checkout did not set the worktree up"
    );
    top
}

fn head(top: &Path) -> String {
    git_text(boards::git(top).args(["rev-parse", "HEAD"]), "read HEAD")
        .unwrap()
        .trim()
        .to_string()
}

/// Commits a file in <top>.
fn commit(top: &Path, file: &str) {
    std::fs::write(top.join(file), file).unwrap();
    git(top, &["add", file]);
    git(top, &["commit", "-q", "-m", file]);
}

use boards::git_text;

#[test]
fn new_hands_out_a_free_worktree_before_adding_one() {
    let scratch = Scratch::new("free-reuse");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["-b", "first"]);
    release(&scratch, 1);
    // Its commit landed on origin/main under another hash, as a rebase does,
    // and origin/main has moved on since.
    commit(&side(&scratch, 1), "landed");
    let sha = head(&side(&scratch, 1));
    git(&main, &["cherry-pick", &sha]);
    commit(&main, "later");
    git(&main, &["push", "-q", "origin", "main"]);

    new(&tool, &main, &["-b", "second", "--for", "agent B"]);
    assert!(
        !side(&scratch, 2).exists(),
        "a second worktree was added while side-1 was free"
    );
    let top = side(&scratch, 1);
    assert_eq!(
        head(&top),
        head(&main),
        "the reused worktree is not at origin/main"
    );
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    assert_eq!(checkout.branch().unwrap().as_deref(), Some("second"));
    assert_eq!(
        Claim::read(&checkout.claim_file()).unwrap().unwrap().who,
        "agent B"
    );
    assert!(top.join("target").is_symlink());
}

#[test]
fn new_adds_a_worktree_when_none_is_free() {
    let scratch = Scratch::new("free-none");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["--for", "agent A"]);
    new(&tool, &main, &["--for", "agent B"]);
    assert!(
        side(&scratch, 2).join(".git").exists(),
        "the claimed side-1 was handed out again"
    );
    let checkout = Checkout::find(&side(&scratch, 2).display().to_string()).unwrap();
    assert_eq!(
        Claim::read(&checkout.claim_file()).unwrap().unwrap().who,
        "agent B"
    );

    // A dirty one is not free, and an app checkout is no side worktree.
    cmd_release(
        &scratch.places(),
        &[side(&scratch, 1).display().to_string()],
    )
    .unwrap();
    cmd_release(
        &scratch.places(),
        &[side(&scratch, 2).display().to_string()],
    )
    .unwrap();
    std::fs::write(side(&scratch, 1).join("stray"), "x").unwrap();
    std::fs::write(side(&scratch, 2).join("stray"), "x").unwrap();
    let app = app_checkout(&tool, &scratch, &main);
    new(&tool, &main, &[]);
    assert!(side(&scratch, 3).exists());
    let checkout = Checkout::find(&app.display().to_string()).unwrap();
    assert!(
        Claim::read(&checkout.claim_file()).unwrap().is_none(),
        "new handed out the app checkout"
    );
}

#[test]
fn a_worktree_with_an_unlanded_commit_is_kept() {
    let scratch = Scratch::new("free-unlanded");
    let (tool, main) = demo(&scratch);
    // side-1 stays claimed, so the work is in side-2, which is not exempt.
    new(&tool, &main, &["--for", "agent"]);
    new(&tool, &main, &["-b", "work", "--for", "agent"]);
    release(&scratch, 2);
    let top = side(&scratch, 2);
    commit(&top, "unlanded");

    retire_free(&tool, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(
        top.exists(),
        "a worktree holding an unlanded commit was retired"
    );
    new(&tool, &main, &[]);
    assert!(
        side(&scratch, 3).exists(),
        "a worktree holding an unlanded commit was handed out"
    );
}

#[test]
fn a_free_worktree_is_retired_at_once_and_the_rest_stay() {
    let scratch = Scratch::new("free-idle");
    let (tool, main) = demo(&scratch);
    // All claimed while they are made, so each `new` adds one.
    for n in 1..=5 {
        new(&tool, &main, &["--for", "agent"]);
        assert!(side(&scratch, n).exists());
    }
    let tops: Vec<_> = (1..=5).map(|n| side(&scratch, n)).collect();
    for n in [1, 2, 3, 5] {
        release(&scratch, n);
    }
    // side-1 is the long-lived one and side-4 stays claimed.
    let app = app_checkout(&tool, &scratch, &main);
    // Each was used a moment ago.

    retire_free(&tool, &tool.roots().unwrap(), Run::Dry).unwrap();
    assert!(tops[1].exists(), "a dry run retired a worktree");
    retire_free(&tool, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(!tops[1].exists(), "a free worktree was kept");
    assert!(
        !tops[2].exists(),
        "a free worktree used a moment ago was kept"
    );
    assert!(!tool.places.root.join("demo/side-2").exists());
    assert!(
        tops[0].exists(),
        "side-1, the long-lived worktree, was retired"
    );
    assert!(tops[3].exists(), "a claimed worktree was retired");
    assert!(!tops[4].exists(), "a free worktree was kept");
    assert!(app.exists(), "the app checkout was retired");
    assert!(main.exists());
}

/// A claim file health cannot read is not an absent claim: retiring fails and
/// the worktree stays, never read as free.
#[test]
fn a_worktree_whose_claim_cannot_be_read_is_not_retired() {
    let scratch = Scratch::new("free-unreadable-claim");
    let (tool, main) = demo(&scratch);
    for _ in 1..=2 {
        new(&tool, &main, &["--for", "agent"]);
    }
    let top = side(&scratch, 2);
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    std::fs::write(checkout.claim_file(), "agent\tnot-a-date\n").unwrap();

    let result = retire_free(&tool, &tool.roots().unwrap(), Run::Real);
    assert!(result.is_err(), "a damaged claim read as no claim");
    assert!(top.exists(), "a worktree with a damaged claim was retired");
}

#[test]
fn two_concurrent_news_never_get_the_same_worktree() {
    let scratch = Scratch::new("free-race");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["--for", "first"]);
    release(&scratch, 1);
    let other = scratch.tool(0);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            cmd_new(
                &tool,
                &crate::testing::no_ssh(),
                &[main.display().to_string(), "--for".into(), "A".into()],
            )
        });
        let b = scope.spawn(|| {
            cmd_new(
                &other,
                &crate::testing::no_ssh(),
                &[main.display().to_string(), "--for".into(), "B".into()],
            )
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    a.unwrap();
    b.unwrap();
    let whos: Vec<String> = (1..=2)
        .map(|n| {
            let checkout = Checkout::find(&side(&scratch, n).display().to_string()).unwrap();
            Claim::read(&checkout.claim_file()).unwrap().unwrap().who
        })
        .collect();
    assert!(
        whos.contains(&"A".to_string()) && whos.contains(&"B".to_string()),
        "{whos:?}"
    );
    assert!(!side(&scratch, 3).exists());
}

#[test]
fn new_without_for_still_claims() {
    let scratch = Scratch::new("free-default-claim");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &[]);
    let checkout = Checkout::find(&side(&scratch, 1).display().to_string()).unwrap();
    assert_eq!(
        Claim::read(&checkout.claim_file()).unwrap().unwrap().who,
        "builds new"
    );
}

/// A side worktree is what `new` and `item take` hand out, so no lane records
/// one as an app checkout: a lane's build would detach the branch of the
/// agent that works there.
#[test]
fn a_side_worktree_is_never_recorded_as_an_app_checkout() {
    let scratch = Scratch::new("free-side-app");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["--for", "agent"]);
    release(&scratch, 1);

    let result = crate::worktree::cmd_app_checkout(
        &tool,
        &crate::testing::no_ssh(),
        &["mac".to_string(), side(&scratch, 1).display().to_string()],
    );
    assert!(
        result.is_err(),
        "a side worktree was recorded as an app checkout"
    );
    assert_eq!(tool.places.app_lane_of("demo", "side-1").unwrap(), None);
}

/// A worktree Claude Code's harness made inside the main checkout, set up the
/// way a build tool would find it: with a build root.
fn harness_worktree(tool: &Tool, main: &Path) -> std::path::PathBuf {
    let top = main.join(".claude/worktrees/agent-abc123");
    git(
        main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &top.display().to_string(),
            "origin/main",
        ],
    );
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    crate::setup::setup(tool, &crate::testing::no_ssh(), &checkout).unwrap();
    top
}

#[test]
fn a_harness_worktree_is_never_handed_out() {
    let scratch = Scratch::new("free-harness-new");
    let (tool, main) = demo(&scratch);
    let harness = harness_worktree(&tool, &main);
    new(&tool, &main, &["--for", "agent"]);
    assert!(
        side(&scratch, 1).join(".git").exists(),
        "new did not add its own side-1"
    );
    let checkout = Checkout::find(&harness.display().to_string()).unwrap();
    assert!(
        Claim::read(&checkout.claim_file()).unwrap().is_none(),
        "a harness worktree was claimed"
    );
    assert!(
        find_free(&tool, &main).unwrap().is_none(),
        "a harness worktree counted as free"
    );
}

#[test]
fn a_harness_worktree_is_never_retired() {
    let scratch = Scratch::new("free-harness-retire");
    let (tool, main) = demo(&scratch);
    let harness = harness_worktree(&tool, &main);
    retire_free(&tool, &tool.roots().unwrap(), Run::Real).unwrap();
    assert!(harness.exists(), "prune retired a harness worktree");
    let error = crate::worktree::cmd_retire(&tool, &[harness.display().to_string()]).unwrap_err();
    assert!(
        harness.exists(),
        "retire removed a harness worktree: {error:?}"
    );
}

#[test]
fn a_harness_worktree_is_never_recorded_as_an_app_checkout() {
    let scratch = Scratch::new("free-harness-app");
    let (tool, main) = demo(&scratch);
    let harness = harness_worktree(&tool, &main);
    let result = crate::worktree::cmd_app_checkout(
        &tool,
        &crate::testing::no_ssh(),
        &["mac".to_string(), harness.display().to_string()],
    );
    assert!(
        result.is_err(),
        "a harness worktree was recorded as the app checkout"
    );
    assert!(
        tool.places
            .app_lane_of("demo", "agent-abc123")
            .unwrap()
            .is_none()
    );
}

#[test]
fn health_does_not_list_a_harness_worktree() {
    let scratch = Scratch::new("free-harness-health");
    let (tool, main) = demo(&scratch);
    let harness = harness_worktree(&tool, &main);
    std::fs::write(harness.join("stray"), "x").unwrap();
    let health = crate::health::health(&tool).unwrap();
    assert!(
        health
            .worktrees
            .iter()
            .all(|worktree| !worktree.path.contains("agent-abc123")),
        "health listed a harness worktree"
    );
}

/// Makes branch <branch> in <main> with one commit of its own, and leaves
/// <main> on main, so no worktree checks the branch out.
fn branch_with_a_commit(main: &Path, branch: &str) {
    git(main, &["switch", "-q", "-c", branch, "origin/main"]);
    commit(main, branch);
    git(main, &["switch", "-q", "main"]);
}

/// `new --resume` checks an existing branch out in the free worktree it hands
/// out, and in the one it adds when none is free, and claims it.
#[test]
fn new_resumes_an_existing_branch_in_a_free_worktree_or_a_new_one() {
    let scratch = Scratch::new("free-resume");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["--for", "first"]);
    release(&scratch, 1);
    branch_with_a_commit(&main, "left");
    branch_with_a_commit(&main, "also-left");

    new(&tool, &main, &["--resume", "left", "--for", "agent A"]);
    new(&tool, &main, &["--resume", "also-left", "--for", "agent B"]);
    for (n, branch, who) in [(1, "left", "agent A"), (2, "also-left", "agent B")] {
        let checkout = Checkout::find(&side(&scratch, n).display().to_string()).unwrap();
        assert_eq!(checkout.branch().unwrap().as_deref(), Some(branch));
        assert!(
            side(&scratch, n).join(branch).is_file(),
            "side-{n} is not at the branch's commit"
        );
        assert_eq!(
            Claim::read(&checkout.claim_file()).unwrap().unwrap().who,
            who
        );
    }
}

/// `new --resume` refuses a branch that does not exist, one another worktree
/// checks out, and a call that also names a new branch with -b, and claims
/// nothing.
#[test]
fn new_refuses_to_resume_a_missing_or_checked_out_branch() {
    let scratch = Scratch::new("free-resume-refused");
    let (tool, main) = demo(&scratch);
    branch_with_a_commit(&main, "left");
    let refused = |extra: &[&str]| {
        let mut args = vec![main.display().to_string()];
        args.extend(extra.iter().map(|arg| arg.to_string()));
        match cmd_new(&tool, &crate::testing::no_ssh(), &args) {
            Err(crate::fail::Fail::Message(message)) => message,
            other => panic!("new did not refuse {extra:?}: {other:?}"),
        }
    };
    assert!(
        refused(&["--resume", "gone"]).contains("no branch gone"),
        "{}",
        refused(&["--resume", "gone"])
    );
    let said = refused(&["--resume", "main"]);
    assert!(
        said.contains(&format!("{} checks out main", main.display())),
        "{said}"
    );
    let said = refused(&["--resume", "left", "-b", "fresh"]);
    assert!(said.contains("-b or --resume"), "{said}");
    assert!(
        !side(&scratch, 1).exists(),
        "a refused new added a worktree"
    );
}

/// `new` hands out no worktree where an item runs, though the worktree has
/// no claim, a clean tree and no commit missing from origin/main: the item's
/// agent works there.
#[test]
fn new_refuses_a_worktree_where_an_item_runs() {
    let demo = crate::lists::item_fixtures::demo("free-running");
    let top = demo.add_side(1, "agent A", "work");
    let worktree = top.display().to_string();
    demo.store
        .write(
            &crate::lists::fixtures::stamp(crate::lists::fixtures::NOON),
            "start",
            |db| {
                let id = crate::lists::fixtures::item(db, vec![]);
                crate::lists::running::start(
                    db,
                    &[id],
                    &[crate::lists::Running {
                        worktree: worktree.clone(),
                        branch: "work".into(),
                        brief: "b.md".into(),
                        since: crate::lists::fixtures::stamp(crate::lists::fixtures::NOON),
                        tier: crate::model_tier::Tier::Implementation,
                    }],
                )
            },
        )
        .unwrap();
    let checkout = Checkout::find(&worktree).unwrap();
    std::fs::remove_file(checkout.claim_file()).unwrap();

    new(&demo.tool, &demo.main, &["--for", "agent B"]);
    assert!(
        demo.main
            .parent()
            .unwrap()
            .join("demo-worktrees/side-2")
            .exists(),
        "new handed out the worktree where an item runs"
    );
    assert_eq!(checkout.branch().unwrap().as_deref(), Some("work"));
    assert!(Claim::read(&checkout.claim_file()).unwrap().is_none());
}

/// `new` hands out no worktree that left a branch holding commits origin/main
/// lacks since its last claim began, though its HEAD sits on origin/main, as
/// a landing leaves it while it builds. Once those commits land, `new` hands
/// the worktree out.
#[test]
fn new_refuses_a_worktree_whose_left_branch_holds_unlanded_commits() {
    let scratch = Scratch::new("free-left-branch");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["--for", "agent A"]);
    new(&tool, &main, &["-b", "work", "--for", "agent B"]);
    let top = side(&scratch, 2);
    commit(&top, "unlanded");
    let sha = head(&top);
    git(&top, &["checkout", "-q", "--detach", "origin/main"]);
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    crate::claim::unclaim(&checkout).unwrap();

    new(&tool, &main, &["--for", "agent C"]);
    assert!(
        side(&scratch, 3).exists(),
        "new handed out a worktree that left a branch with unlanded commits"
    );
    let work = git_text(boards::git(&main).args(["rev-parse", "work"]), "read work").unwrap();
    assert_eq!(work.trim(), sha, "new moved the branch that holds the work");

    git(&main, &["cherry-pick", &sha]);
    git(&main, &["push", "-q", "origin", "main"]);
    new(&tool, &main, &["--for", "agent D"]);
    assert!(
        !side(&scratch, 4).exists(),
        "new added a worktree while side-2's work had landed"
    );
    assert_eq!(
        Claim::read(&checkout.claim_file()).unwrap().unwrap().who,
        "agent D"
    );
}

/// `new -b` with the name of a branch that holds commits origin/main lacks
/// refuses, and the branch stays at its commit: new moves no branch.
#[test]
fn new_never_moves_a_branch_that_holds_work() {
    let scratch = Scratch::new("free-branch-taken");
    let (tool, main) = demo(&scratch);
    new(&tool, &main, &["--for", "first"]);
    release(&scratch, 1);
    branch_with_a_commit(&main, "taken");
    let before = git_text(
        boards::git(&main).args(["rev-parse", "taken"]),
        "read taken",
    )
    .unwrap();

    let result = cmd_new(
        &tool,
        &crate::testing::no_ssh(),
        &[
            main.display().to_string(),
            "-b".into(),
            "taken".into(),
            "--for".into(),
            "agent".into(),
        ],
    );
    assert!(result.is_err(), "new took a branch name that exists");
    let after = git_text(
        boards::git(&main).args(["rev-parse", "taken"]),
        "read taken",
    )
    .unwrap();
    assert_eq!(after, before, "new moved the branch");
    let checkout = Checkout::find(&side(&scratch, 1).display().to_string()).unwrap();
    assert!(
        Claim::read(&checkout.claim_file()).unwrap().is_none(),
        "a refused new kept its claim"
    );
}
