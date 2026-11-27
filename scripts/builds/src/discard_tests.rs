use super::*;
use crate::command::Command;
use crate::lists::item_fixtures::{Demo, demo, words};

fn discard(demo: &Demo, args: &[&str]) -> Result<()> {
    let args = Args::of(Command::Discard, &words(args)).unwrap();
    cmd_discard(&demo.tool, &args, &demo.scratch.dir)
}

fn refusal(result: Result<()>) -> String {
    result.unwrap_err().text()
}

fn branch_tip(demo: &Demo, branch: &str) -> Option<String> {
    crate::git::branch_tip(&demo.main, branch).unwrap()
}

/// The one decision the project's log holds.
fn recorded(demo: &Demo) -> crate::lists::decision::Decision {
    let mut decisions = demo.store.load().unwrap().decisions;
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    decisions.remove(0)
}

/// Discarding a worktree throws away its commits, the commits of a branch it
/// left and its files not committed, frees it, and records who did it, why,
/// and each commit and file.
#[test]
fn discard_throws_away_and_records_a_worktrees_work() {
    let demo = demo("discard-worktree");
    let side = demo.add_side(1, "agent A", "first");
    let left = demo.commit(&side, "left-behind");
    crate::testing::git(&side, &["checkout", "-q", "-b", "second", "main"]);
    let head = demo.commit(&side, "unlanded");
    std::fs::write(side.join("loose"), "x").unwrap();
    let side_arg = side.display().to_string();

    discard(&demo, &[&side_arg, "--why", "a newer branch replaces it"]).unwrap();

    let checkout = Checkout::find(&side_arg).unwrap();
    assert_eq!(checkout.branch().unwrap(), None);
    assert!(!side.join("loose").exists());
    assert_eq!(branch_tip(&demo, "first"), None);
    assert_eq!(branch_tip(&demo, "second"), None);
    assert!(
        crate::claim::Claim::read(&checkout.claim_file())
            .unwrap()
            .is_none()
    );
    let decision = recorded(&demo);
    assert_eq!(decision.by, "tester");
    assert_eq!(
        decision.body,
        format!(
            "a newer branch replaces it\n\ncommit on second: {head} unlanded\ncommit on first: {left} left-behind\nfile not committed: loose"
        )
    );
}

#[test]
fn discard_deletes_and_records_a_branch_no_worktree_checks_out() {
    let demo = demo("discard-branch");
    let side = demo.add_side(1, "agent A", "stray");
    let sha = demo.commit(&side, "unlanded");
    let main_arg = demo.main.display().to_string();
    let words = [main_arg.as_str(), "--branch", "stray", "--why", "dead end"];

    assert!(refusal(discard(&demo, &words)).contains("checks it out"));
    crate::testing::git(&side, &["checkout", "-q", "--detach", "main"]);
    discard(&demo, &words).unwrap();

    assert_eq!(branch_tip(&demo, "stray"), None);
    assert_eq!(
        recorded(&demo).body,
        format!("dead end\n\ncommit on stray: {sha} unlanded")
    );
}

/// An item's work goes with `item drop`, and a discard with no why or with
/// nothing to throw away changes nothing.
#[test]
fn discard_refuses_an_items_work_no_why_and_no_work() {
    let demo = demo("discard-refused");
    let side = demo.add_side(1, "agent A", "work");
    let side_arg = side.display().to_string();
    assert!(
        refusal(discard(&demo, &[&side_arg, "--why", "w"])).contains("holds no work to throw away")
    );
    let sha = demo.commit(&side, "unlanded");
    let no_why = Args::of(Command::Discard, &words(&[&side_arg])).unwrap_err();
    assert!(no_why.text().contains("--why is required"));

    let item = |args: &[&str]| {
        crate::lists::item_words(&demo.tool, &crate::testing::no_ssh(), &words(args)).unwrap()
    };
    let details = demo.brief("details.txt");
    let brief = demo.brief("b.md");
    item(&[
        "add",
        "--title",
        "Do the first thing now",
        "--area",
        "Alpha",
        "--details-file",
        &details,
    ]);
    item(&[
        "take",
        "1",
        "--worktree",
        &side_arg,
        "--branch",
        "work",
        "--brief",
        &brief,
    ]);
    assert!(refusal(discard(&demo, &[&side_arg, "--why", "w"])).contains("builds item drop"));
    assert_eq!(branch_tip(&demo, "work"), Some(sha));
    assert!(demo.store.load().unwrap().decisions.is_empty());
}
