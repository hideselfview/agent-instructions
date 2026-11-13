use super::*;
use crate::lists::item_command::cmd_item;
use crate::lists::item_fixtures::{Demo, demo, refusal, words};
use crate::lists::Record;
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
    let before = l.demo.state();
    assert!(refusal(check(&l, &[&l.commit])).contains("name the work items"));
    assert!(refusal(check(&l, &["--done", "1", "--part", "1", &l.commit])).contains("named twice"));
    assert!(refusal(check(&l, &["--done", "9", &l.commit])).contains("not a record"));
    assert!(refusal(check(&l, &["--done", "1", "deadbeef"])).contains("not a commit"));
    // A commit on main only is on no item's branch.
    let on_main = l.demo.commit(&l.lander, "m.txt");
    assert!(refusal(check(&l, &["--done", "1", &on_main])).contains("on the branch of none"));
    cmd_item(&l.demo.tool, &words(&["stop", "2", "--why", "x"])).unwrap();
    assert!(refusal(check(&l, &["--done", "2", &l.commit])).contains("is not running"));
    assert_ne!(l.demo.state(), before); // the stop above
    let before = l.demo.state();
    check(&l, &["--done", "1", &l.commit]).unwrap();
    assert_eq!(l.demo.state(), before, "check-landing changed the store");
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

    let after_first = l.demo.state();
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

/// What `item show` prints for item <id>.
fn shown(l: &Landing, id: u32) -> String {
    crate::lists::item_show::render_item(&l.demo.store.load().unwrap(), Id(id)).unwrap()
}

#[test]
fn landed_done_ends_where_the_item_runs_and_a_part_leaves_it_running() {
    let l = setup("landing-done-stops-running");
    push(&l);
    assert!(shown(&l, 2).contains("running since"), "the item runs before it lands");
    landed(&l, &["--part", "1", "--done", "2", &l.commit]).unwrap();
    let lists = l.demo.store.load().unwrap();
    let Record::Item(closed) = &lists.closed_record(Id(2)).expect("item 2 closed").record else { panic!("an item") };
    assert!(closed.running.is_empty(), "a closed item runs nowhere: {:?}", closed.running);
    assert_eq!(lists.item(Id(1)).unwrap().running.len(), 1, "a part leaves the item running");
}

#[test]
fn item_show_of_a_landed_item_prints_no_running_marker() {
    let l = setup("landing-done-show");
    push(&l);
    landed(&l, &["--done", "2", &l.commit]).unwrap();
    let text = shown(&l, 2);
    assert!(text.contains("[closed, landed"), "{text}");
    assert!(!text.contains("Running:") && !text.contains("running since"), "{text}");
}

#[test]
fn landed_refuses_a_different_set_for_a_closed_item_and_commits_not_on_main() {
    let l = setup("landing-landed-refuse");
    assert!(refusal(landed(&l, &["--done", "1", &l.commit])).contains("not on origin/main"));
    push(&l);
    landed(&l, &["--done", "1", &l.commit]).unwrap();
    let second = l.demo.commit(&l.agent, "b.txt");
    push(&l);
    let before = l.demo.state();
    let refused = refusal(landed(&l, &["--done", "1", &second]));
    assert!(refused.contains("closed as landed with other commits"), "{refused}");
    assert_eq!(l.demo.state(), before);
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

/// An item whose work spans two repos: running in side-1 of `demo` on `work`
/// and side-1 of `other` on `work-other`, each repo with one commit on its
/// branch and a detached landing worktree (side-2).
struct TwoRepos {
    demo: Demo,
    demo_lander: std::path::PathBuf,
    other_lander: std::path::PathBuf,
    demo_agent: std::path::PathBuf,
    other_agent: std::path::PathBuf,
    demo_commit: String,
    other_commit: String,
}

fn two_repos(name: &str) -> TwoRepos {
    let demo = crate::lists::item_fixtures::demo_over(name, &["demo", "other"]);
    let demo_agent = demo.add_side_of("demo", 1, "agent A", "work");
    let other_agent = demo.add_side_of("other", 1, "agent A", "work-other");
    let demo_lander = demo.add_side_of("demo", 2, "coordinator", "landing");
    let other_lander = demo.add_side_of("other", 2, "coordinator", "landing-other");
    run_git(&demo_lander, &["checkout", "-q", "--detach", "main"]);
    run_git(&other_lander, &["checkout", "-q", "--detach", "main"]);
    let details = demo.brief("details.txt");
    let brief = demo.brief("b.md");
    cmd_item(&demo.tool, &words(&["add", "--title", "Do the first thing now", "--area", "Alpha", "--details-file", &details])).unwrap();
    cmd_item(
        &demo.tool,
        &words(&[
            "take",
            "1",
            "--worktree",
            demo_agent.to_str().unwrap(),
            "--branch",
            "work",
            "--worktree",
            other_agent.to_str().unwrap(),
            "--branch",
            "work-other",
            "--brief",
            &brief,
        ]),
    )
    .unwrap();
    let demo_commit = demo.commit(&demo_agent, "a.txt");
    let other_commit = demo.commit(&other_agent, "b.txt");
    TwoRepos { demo, demo_lander, other_lander, demo_agent, other_agent, demo_commit, other_commit }
}

fn land_args(lander: &std::path::Path, extra: &[&str]) -> Vec<String> {
    let mut args = vec![lander.display().to_string()];
    args.extend(extra.iter().map(|a| a.to_string()));
    args
}

#[test]
fn an_item_in_two_repos_is_taken_in_both_and_shown_in_both() {
    let t = two_repos("landing-two-take");
    let lists = t.demo.store.load().unwrap();
    let running = &lists.item(Id(1)).unwrap().running;
    assert_eq!(
        running.iter().map(|r| (r.worktree.clone(), r.branch.clone())).collect::<Vec<_>>(),
        vec![
            (t.demo_agent.display().to_string(), "work".to_string()),
            (t.other_agent.display().to_string(), "work-other".to_string())
        ]
    );
    let shown = crate::lists::item_show::render_item(&lists, Id(1)).unwrap();
    assert!(shown.contains("Running: agent A, demo side-1 work"), "{shown}");
    assert!(shown.contains("Running: agent A, other side-1 work-other"), "{shown}");
    let line = crate::lists::show::item_line(&lists, lists.item(Id(1)).unwrap());
    assert!(line.contains(&t.demo_agent.display().to_string()) && line.contains(&t.other_agent.display().to_string()), "{line}");
    // The view the dashboard reads lists both.
    let view = serde_json::to_value(crate::lists::view::View::build(&lists, crate::lists::fixtures::NOON)).unwrap();
    let repos: Vec<&str> = view["items"][0]["running"].as_array().unwrap().iter().map(|r| r["repo"].as_str().unwrap()).collect();
    assert_eq!(repos, ["demo", "other"]);
}

#[test]
fn take_refuses_two_worktrees_of_one_repo_and_unpaired_branches() {
    let demo = demo("landing-two-same-repo");
    let one = demo.add_side(1, "agent A", "work");
    let two = demo.add_side(2, "agent A", "work2");
    let details = demo.brief("details.txt");
    let brief = demo.brief("b.md");
    cmd_item(&demo.tool, &words(&["add", "--title", "Do the first thing now", "--area", "Alpha", "--details-file", &details])).unwrap();
    let before = demo.state();
    let (one, two) = (one.to_str().unwrap(), two.to_str().unwrap());
    let same = refusal(cmd_item(
        &demo.tool,
        &words(&["take", "1", "--worktree", one, "--branch", "work", "--worktree", two, "--branch", "work2", "--brief", &brief]),
    ));
    assert!(same.contains("both worktrees of demo"), "{same}");
    let unpaired = refusal(cmd_item(&demo.tool, &words(&["take", "1", "--worktree", one, "--worktree", two, "--branch", "work", "--brief", &brief])));
    assert!(unpaired.contains("2 --worktree and 1 --branch"), "{unpaired}");
    assert_eq!(demo.state(), before);
}

#[test]
fn a_part_from_one_repo_leaves_the_item_running_and_done_from_the_other_closes_it() {
    let t = two_repos("landing-two-land");
    run_git(&t.demo_agent, &["push", "-q", "origin", "HEAD:main"]);
    run_git(&t.demo_lander, &["fetch", "-q", "origin"]);
    run_git(&t.other_agent, &["push", "-q", "origin", "HEAD:main"]);
    run_git(&t.other_lander, &["fetch", "-q", "origin"]);

    // Each repo's landing is accepted by the check.
    let demo_check = land_args(&t.demo_lander, &["--part", "1", &t.demo_commit]);
    check_landing(&t.demo.tool, &demo_check).unwrap();
    let other_check = land_args(&t.other_lander, &["--done", "1", &t.other_commit]);
    check_landing(&t.demo.tool, &other_check).unwrap();

    super::landed(&t.demo.tool, &demo_check, &t.demo_lander).unwrap();
    let lists = t.demo.store.load().unwrap();
    let item = lists.item(Id(1)).unwrap();
    assert_eq!((item.running.len(), item.landed.len(), item.landed[0].repo.as_str()), (2, 1, "demo"));

    super::landed(&t.demo.tool, &other_check, &t.other_lander).unwrap();
    let lists = t.demo.store.load().unwrap();
    assert!(lists.item(Id(1)).is_none());
    assert!(matches!(&lists.closed[0].how, How::Landed(c) if *c == [t.other_commit.clone()]));
    let Record::Item(closed) = &lists.closed[0].record else { panic!("an item") };
    assert!(closed.running.is_empty(), "done ends the running in every repo: {:?}", closed.running);
}

#[test]
fn a_landing_from_the_second_repo_is_accepted_and_the_first_repos_commit_is_not_in_it() {
    let t = two_repos("landing-two-second-repo");
    let from_other = land_args(&t.other_lander, &["--part", "1", &t.other_commit]);
    check_landing(&t.demo.tool, &from_other).unwrap();
    let wrong_commit = land_args(&t.other_lander, &["--part", "1", &t.demo_commit]);
    let refused = refusal(check_landing(&t.demo.tool, &wrong_commit));
    assert!(refused.contains("not a commit"), "{refused}");
}

#[test]
fn every_named_constraint_of_the_landing_tables_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, item, row, sha, v};
    let db = db();
    let id = item(&db, vec![]);
    add_part(&db, id, LandedPart { repo: "bae".into(), commits: vec![sha('1')], at: 5 }).unwrap();
    let landing: i64 = db.query_row("SELECT id FROM item_landing", [], |r| r.get(0)).unwrap();
    let commit = [("landing_id", v(landing)), ("position", v(3)), ("sha", v(sha('2')))];
    let mut refused = Refusals::new(include_str!("landing.sql"));
    refused.check("item_landing_commit_position_not_negative", plant(&db, "item_landing_commit", &row(&commit, &[("position", v(-1))])));
    for bad in ["abc", &"A".repeat(40), &"1".repeat(41)] {
        refused.check("item_landing_commit_sha_is_a_commit", plant(&db, "item_landing_commit", &row(&commit, &[("sha", v(bad.to_string()))])));
    }
    refused.check_as(
        "item_landing_commit_once",
        "UNIQUE constraint failed: item_landing_commit.landing_id, item_landing_commit.sha",
        plant(&db, "item_landing_commit", &row(&commit, &[("sha", v(sha('1')))])),
    );
    plant(&db, "item_landing_commit", &commit).unwrap();
    refused.done();
}
