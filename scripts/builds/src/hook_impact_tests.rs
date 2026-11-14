use super::*;
use crate::checks::Kind;
use crate::hook_stats::{CheckRecord, DATABASE, Outcome, Run, record};
use crate::testing::{Scratch, git, repo};

fn run(at: i64, base: Option<&str>, total_ms: u64, clippy_ms: u64, crates: u32) -> Run {
    Run {
        at,
        repo: "bae".into(),
        worktree: "/d/bae".into(),
        branch: Some("main".into()),
        base: base.map(str::to_string),
        passed: true,
        plan_ms: 1,
        total_ms,
        files: vec![],
        checks: vec![
            CheckRecord {
                name: "clippy".into(),
                kind: Kind::Check,
                outcome: Outcome::Passed,
                handed: Some((1, crates)),
                ms: Some(clippy_ms),
            },
            CheckRecord {
                name: "fmt".into(),
                kind: Kind::Fix,
                outcome: Outcome::Skipped,
                handed: None,
                ms: None,
            },
        ],
    }
}

fn head(dir: &Path) -> String {
    git_text(&mut git_in(dir, &["rev-parse", "HEAD"]), "read HEAD")
        .unwrap()
        .trim()
        .to_string()
}

fn git_in(dir: &Path, args: &[&str]) -> std::process::Command {
    let mut command = crate::checkout::git(dir);
    command.args(args);
    command
}

#[test]
fn runs_split_by_the_base_commit_containing_the_change_even_on_an_old_base_after_the_landing() {
    let scratch = Scratch::new("impact-git");
    let top = scratch.dir.join("repo");
    repo(&top, "target 1\n");
    let old = head(&top);
    std::fs::write(top.join("change.txt"), "c").unwrap();
    git(&top, &["add", "change.txt"]);
    git(&top, &["commit", "-q", "-m", "The change"]);
    let change = head(&top);
    std::fs::write(top.join("after.txt"), "a").unwrap();
    git(&top, &["add", "after.txt"]);
    git(&top, &["commit", "-q", "-m", "After"]);
    let new = head(&top);
    let (_, committed_at) = commit(&top, &change).unwrap();
    // The folder holds the repo's own worktree path so the runs can ask git.
    let path = scratch.dir.join(DATABASE);
    let at = |when: i64, base: Option<&str>, total, clippy, crates| {
        let mut r = run(when, base, total, clippy, crates);
        r.worktree = top.to_string_lossy().into_owned();
        record(&path, &r).unwrap();
    };
    at(committed_at - 100, Some(&old), 100_000, 80_000, 8);
    at(committed_at - 50, None, 120_000, 90_000, 8);
    // After the landing, a worktree still on the old base runs the old code.
    at(committed_at + 100, Some(&old), 110_000, 85_000, 8);
    at(committed_at + 200, Some(&new), 30_000, 10_000, 1);
    at(committed_at + 300, Some(&change), 40_000, 14_000, 1);
    at(committed_at + 400, None, 50_000, 20_000, 1);
    at(
        committed_at + 500,
        Some("0123456789012345678901234567890123456789"),
        50_000,
        20_000,
        1,
    );
    let impact = impact(&path, &change[..10]).unwrap().unwrap();
    assert_eq!(impact.change, change);
    assert_eq!(
        (
            impact.with.runs,
            impact.with.median_ms,
            impact.with.slowest_ms
        ),
        (2, 35_000, 40_000)
    );
    assert_eq!(
        (
            impact.without.runs,
            impact.without.median_ms,
            impact.without.slowest_ms
        ),
        (3, 110_000, 120_000)
    );
    assert_eq!(
        (
            impact.without_by_time,
            impact.unplaced_by_time,
            impact.unplaced_by_base
        ),
        (1, 1, 1)
    );
    let clippy = |side: &Side| {
        side.checks
            .iter()
            .find(|c| c.name == "clippy")
            .map(|c| (c.ran, c.median_ms, c.slowest_ms, c.median_crates))
            .unwrap()
    };
    assert_eq!(clippy(&impact.with), (2, 12_000, 14_000, 1));
    assert_eq!(clippy(&impact.without), (3, 85_000, 90_000, 8));
    assert!(
        impact.with.checks.iter().all(|c| c.name != "fmt"),
        "a skipped check did not run"
    );
    let text = report("bae", Some(&impact));
    assert!(
        text.contains("with the change:    2 runs median 35.0s, slowest 40.0s"),
        "{text}"
    );
    assert!(
        text.contains("before the hook kept its base commit")
            && text.contains("base commit is not in the repo"),
        "{text}"
    );
}

#[test]
fn a_change_with_no_runs_recorded_is_none() {
    let scratch = Scratch::new("impact-none");
    assert_eq!(
        split(&scratch.dir.join(DATABASE), "c", 1, &|_| Some(true)).unwrap(),
        None
    );
    assert!(report("bae", None).contains("no runs recorded"));
}

#[test]
fn a_revision_that_starts_with_a_dash_is_refused() {
    let scratch = Scratch::new("impact-dash");
    let top = scratch.dir.join("repo");
    repo(&top, "target 1\n");
    assert!(
        commit(&top, "--output=x")
            .unwrap_err()
            .text()
            .contains("not a revision")
    );
}
