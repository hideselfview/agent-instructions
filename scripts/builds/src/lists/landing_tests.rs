use super::*;
use crate::lists::item::cmd_item;
use crate::lists::item_fixtures::{Demo, demo, refusal, words};
use crate::testing::git as run_git;

/// A repo with agent A running items 1 and 2 on branch `work` in side-1, one
/// commit on it, and side-2 as the landing worktree.
struct Landing {
    demo: Demo,
    agent: std::path::PathBuf,
    lander: std::path::PathBuf,
    commit: String,
}

fn setup(name: &str) -> Landing {
    let demo = demo(name);
    let agent = demo.add_side(1, "agent A", "work");
    let lander = demo.add_side(2, "coordinator (landing)", "landing");
    run_git(&lander, &["checkout", "-q", "--detach", "main"]);
    let details = demo.brief("details.txt");
    let brief = demo.brief("b.md");
    for title in ["Do the first thing now", "Do the second thing now"] {
        cmd_item(&demo.tool, &words(&["add", "--title", title, "--area", "Alpha", "--details-file", &details])).unwrap();
    }
    cmd_item(&demo.tool, &words(&["take", "1", "2", "--worktree", agent.to_str().unwrap(), "--branch", "work", "--brief", &brief])).unwrap();
    let commit = demo.commit(&agent, "a.txt");
    Landing { demo, agent, lander, commit }
}

fn check(l: &Landing, extra: &[&str]) -> Result<()> {
    let mut args = vec!["check-landing", l.lander.to_str().unwrap()];
    args.extend_from_slice(extra);
    cmd_item(&l.demo.tool, &words(&args))
}

fn landed(l: &Landing, extra: &[&str]) -> Result<()> {
    let mut args = vec!["landed", l.lander.to_str().unwrap()];
    args.extend_from_slice(extra);
    // land.sh runs it inside the landing worktree.
    let _ = &l.agent;
    let cwd = l.lander.clone();
    landed_in(l, &args[1..], &cwd)
}

fn landed_in(l: &Landing, args: &[&str], cwd: &Path) -> Result<()> {
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    super::landed(&l.demo.tool, &owned, cwd)
}

/// Puts the commit on origin/main and fetches it into the landing worktree,
/// as the push does.
fn push(l: &Landing) {
    run_git(&l.agent, &["push", "-q", "origin", "HEAD:main"]);
    run_git(&l.lander, &["fetch", "-q", "origin"]);
}

fn bare(l: &Landing, args: &[&str]) -> Result<()> {
    let mut all = vec!["check-landing", l.lander.to_str().unwrap()];
    all.extend_from_slice(args);
    let owned: Vec<String> = all.iter().map(|a| a.to_string()).collect();
    check_landing(&l.demo.tool, &owned[1..])
}

#[test]
fn check_landing_takes_land_shs_arguments_and_passes_a_branch_commit() {
    let l = setup("landing-check");
    check(&l, &["--done", "1", "--part", "2", &l.commit]).unwrap();
    check(&l, &["--part", "1", &l.commit[..10]]).unwrap();
    // The form land.sh uses carries no --project and no --by.
    bare(&l, &["--done", "1", &l.commit]).unwrap();
}

#[test]
fn check_landing_refuses_each_thing_it_requires() {
    let l = setup("landing-check-refuse");
    let before = l.demo.bytes();
    assert!(refusal(check(&l, &[&l.commit])).contains("name the work items"));
    assert!(refusal(check(&l, &["--done", "1", "--part", "1", &l.commit])).contains("named twice"));
    assert!(refusal(check(&l, &["--done", "9", &l.commit])).contains("not a record"));
    assert!(refusal(check(&l, &["--done", "1", "deadbeef"])).contains("not a commit"));
    // A commit on main only is on no item's branch.
    let on_main = l.demo.commit(&l.lander, "m.txt");
    assert!(refusal(check(&l, &["--done", "1", &on_main])).contains("on the branch of none"));
    cmd_item(&l.demo.tool, &words(&["stop", "2", "--why", "x"])).unwrap();
    assert!(refusal(check(&l, &["--done", "2", &l.commit])).contains("is not running"));
    assert_ne!(l.demo.bytes(), before); // the stop above
    let before = l.demo.bytes();
    check(&l, &["--done", "1", &l.commit]).unwrap();
    assert_eq!(l.demo.bytes(), before, "check-landing changed the store");
}

#[test]
fn landed_records_a_part_then_closes_and_is_idempotent() {
    let l = setup("landing-landed");
    push(&l);
    landed(&l, &["--part", "1", "--done", "2", &l.commit]).unwrap();
    let lists = l.demo.store.load().unwrap();
    let one = lists.item(Id(1)).unwrap();
    assert_eq!((one.landed.len(), one.landed[0].repo.as_str(), one.landed[0].commits.clone()), (1, "demo", vec![l.commit.clone()]));
    assert!(lists.item(Id(2)).is_none());
    assert!(matches!(&lists.closed[0].how, How::Landed(c) if *c == [l.commit.clone()]));

    let after_first = l.demo.bytes();
    let version = lists.version;
    // The same call again records once; it changes nothing (not even the version).
    let again = landed(&l, &["--part", "1", "--done", "2", &l.commit]);
    again.unwrap();
    let lists = l.demo.store.load().unwrap();
    assert_eq!(lists.item(Id(1)).unwrap().landed.len(), 1);
    assert_eq!(lists.closed.len(), 1);
    assert_eq!(lists.version, version + 1, "the repeat is a logged write that adds no record");
    let _ = after_first;
}

#[test]
fn landed_refuses_a_different_set_for_a_closed_item_and_commits_not_on_main() {
    let l = setup("landing-landed-refuse");
    assert!(refusal(landed(&l, &["--done", "1", &l.commit])).contains("not on origin/main"));
    push(&l);
    landed(&l, &["--done", "1", &l.commit]).unwrap();
    let second = l.demo.commit(&l.agent, "b.txt");
    push(&l);
    let before = l.demo.bytes();
    let refused = refusal(landed(&l, &["--done", "1", &second]));
    assert!(refused.contains("closed as landed with other commits"), "{refused}");
    assert_eq!(l.demo.bytes(), before);
    let dropped = refusal(landed(&l, &["--done", "7", &second]));
    assert!(dropped.contains("not a record"), "{dropped}");
}

#[test]
fn landed_in_another_repo_than_the_item_runs_in_is_refused() {
    let l = setup("landing-wrong-repo");
    push(&l);
    // Item 1 runs in demo's side-1; make the landing repo another one.
    let other = l.demo.scratch.dir.join("other");
    crate::testing::repo(&other, "target 1G\n");
    let origin = l.demo.scratch.dir.join("origin2.git");
    run_git(&l.demo.scratch.dir, &["init", "-q", "--bare", &origin.display().to_string()]);
    run_git(&other, &["remote", "add", "origin", &origin.display().to_string()]);
    run_git(&other, &["push", "-q", "origin", "main"]);
    let args: Vec<String> = ["--done", "1", "--project", "demo", "--by", "t", "HEAD"].iter().map(|s| s.to_string()).collect();
    let mut with_dir = vec![other.display().to_string()];
    with_dir.extend(args);
    let refused = refusal(super::landed(&l.demo.tool, &with_dir, &other));
    assert!(refused.contains("not in a worktree of other"), "{refused}");
}
