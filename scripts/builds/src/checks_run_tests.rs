use super::*;
use crate::checks::parse;
use crate::hook_stats::{Change, Outcome, Run};
use crate::testing::{Scratch, git};

/// A repo with <files> committed, a `.checks` of <checks> (committed too), and
/// stub programs in tools/ that append their arguments to `log`.
fn repo(scratch: &Scratch, checks: &str) -> std::path::PathBuf {
    let top = scratch.dir.join("repo");
    std::fs::create_dir_all(top.join("tools")).unwrap();
    git(&top, &["init", "-q", "-b", "main"]);
    git(&top, &["config", "user.email", "test@example.com"]);
    git(&top, &["config", "user.name", "test"]);
    stub(&top, "ok.sh", "echo \"ok $*\" >> \"$(git rev-parse --show-toplevel)/log\"\n");
    stub(&top, "second.sh", "echo \"second $*\" >> \"$(git rev-parse --show-toplevel)/log\"\n");
    stub(&top, "fail.sh", "echo \"fail $*\" >> \"$(git rev-parse --show-toplevel)/log\"\necho boom >&2\nexit 3\n");
    stub(&top, "format.sh", "for f in \"$@\"; do printf 'formatted\\n' > \"$f\"; done\n");
    std::fs::write(top.join(".checks"), checks).unwrap();
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "Start"]);
    top
}

fn stub(top: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = top.join("tools").join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn stage(top: &Path, name: &str, content: &str) {
    std::fs::write(top.join(name), content).unwrap();
    git(top, &["add", name]);
}

fn log(top: &Path) -> String {
    std::fs::read_to_string(top.join("log")).unwrap_or_default()
}

fn staged_content(top: &Path, name: &str) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(top).args(["show", &format!(":{name}")]).output().unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn a_run_with_nothing_matching_runs_nothing_and_succeeds() {
    let scratch = Scratch::new("run-none");
    let top = repo(&scratch, "check sh *.sh tools/ok.sh {files}\n");
    stage(&top, "notes.md", "x");
    run_staged(&top, None).unwrap();
    assert_eq!(log(&top), "");
}

#[test]
fn a_check_gets_the_staged_files_it_matches() {
    let scratch = Scratch::new("run-files");
    let top = repo(&scratch, "check sh *.sh tools/ok.sh {files}\ncheck md *.md tools/second.sh\n");
    stage(&top, "a.sh", "x");
    stage(&top, "b.md", "x");
    run_staged(&top, None).unwrap();
    assert_eq!(log(&top), "ok a.sh\nsecond \n");
}

#[test]
fn a_failing_check_still_lets_the_rest_run_and_the_run_fails() {
    let scratch = Scratch::new("run-fail");
    let top = repo(&scratch, "check bad *.sh tools/fail.sh\ncheck good *.sh tools/second.sh\n");
    stage(&top, "a.sh", "x");
    let error = run_staged(&top, None).unwrap_err().text();
    assert_eq!(log(&top), "fail \nsecond \n");
    assert!(error.contains("bad") && !error.contains("good"), "{error}");
}

#[test]
fn a_fix_stages_again_the_files_it_changed() {
    let scratch = Scratch::new("run-fix");
    let top = repo(&scratch, "fix fmt *.txt tools/format.sh {files}\n");
    stage(&top, "a.txt", "ugly");
    run_staged(&top, None).unwrap();
    assert_eq!(staged_content(&top, "a.txt"), "formatted\n");
    let out = std::process::Command::new("git").arg("-C").arg(&top).args(["diff", "--name-only"]).output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "");
}

#[test]
fn a_fix_refuses_a_changed_file_that_had_unstaged_changes_and_stages_nothing() {
    let scratch = Scratch::new("run-partial");
    let top = repo(&scratch, "fix fmt *.txt tools/format.sh {files}\n");
    stage(&top, "a.txt", "staged");
    stage(&top, "b.txt", "other");
    std::fs::write(top.join("a.txt"), "staged\nmore, not staged").unwrap();
    let error = run_staged(&top, None).unwrap_err().text();
    assert!(error.contains("a.txt") && error.contains("not staged"), "{error}");
    assert_eq!(staged_content(&top, "a.txt"), "staged");
    assert_eq!(staged_content(&top, "b.txt"), "other");
}

#[test]
fn a_fix_that_leaves_a_partly_staged_file_alone_is_fine() {
    let scratch = Scratch::new("run-partial-same");
    let top = repo(&scratch, "fix fmt *.txt tools/second.sh {files}\n");
    stage(&top, "a.txt", "staged");
    std::fs::write(top.join("a.txt"), "staged\nmore").unwrap();
    run_staged(&top, None).unwrap();
    assert_eq!(staged_content(&top, "a.txt"), "staged");
}

#[test]
fn list_says_what_run_would_run() {
    let scratch = Scratch::new("run-list");
    let top = repo(&scratch, "check sh *.sh tools/ok.sh {files}\nfix md *.md tools/format.sh {files}\ntest t *.sh tools/ok.sh\n");
    stage(&top, "a.sh", "x");
    let text = list_staged(&top).unwrap();
    assert_eq!(text, "check sh: tools/ok.sh a.sh\n");
    assert_eq!(log(&top), "");
}

fn recorded(scratch: &Scratch) -> std::path::PathBuf {
    scratch.dir.join("builds.sqlite")
}

fn recorded_runs(log: &Path) -> Vec<Run> {
    crate::hook_stats::stats(log, 0).unwrap().expect("something was recorded").recent
}

#[test]
fn a_recorded_run_holds_the_files_each_check_with_its_time_and_what_was_skipped() {
    let scratch = Scratch::new("run-record");
    let top = repo(
        &scratch,
        "check slow *.sh tools/slow.sh {files}\ncheck fail-me *.sh tools/fail.sh\ncheck docs *.md tools/ok.sh {files}\n",
    );
    stub(&top, "slow.sh", "sleep 0.3\n");
    stage(&top, "gone.sh", "old\n");
    git(&top, &["add", "tools/slow.sh"]);
    git(&top, &["commit", "-q", "-m", "Add slow"]);
    git(&top, &["rm", "-q", "gone.sh"]);
    stage(&top, "a.sh", "one\ntwo\nthree\n");
    stage(&top, "Makefile", "all:\n");
    std::fs::write(top.join("image.bin"), [0u8, 1, 2, 0]).unwrap();
    git(&top, &["add", "image.bin"]);
    let log = recorded(&scratch);
    assert!(run_staged(&top, Some(&log)).is_err());
    let runs = recorded_runs(&log);
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!((run.passed, run.repo.as_str(), run.branch.as_deref()), (false, "repo", Some("main")));
    assert_eq!(run.worktree, top.to_string_lossy());
    let files: Vec<_> = run.files.iter().map(|f| (f.path.as_str(), f.change, f.lines)).collect();
    assert_eq!(
        files,
        vec![
            ("Makefile", Change::Added, Some((1, 0))),
            ("a.sh", Change::Added, Some((3, 0))),
            ("gone.sh", Change::Deleted, Some((0, 1))),
            ("image.bin", Change::Added, None),
        ]
    );
    let checks: Vec<_> = run.checks.iter().map(|c| (c.name.as_str(), c.outcome, c.handed)).collect();
    assert_eq!(
        checks,
        vec![("slow", Outcome::Passed, Some((1, 0))), ("fail-me", Outcome::Failed, Some((0, 0))), ("docs", Outcome::Skipped, None)]
    );
    let slow_ms = run.checks[0].ms.unwrap();
    assert!(slow_ms >= 300, "{slow_ms}");
    assert_eq!(run.checks[2].ms, None);
    assert!(run.total_ms >= run.plan_ms + slow_ms, "{} {} {slow_ms}", run.total_ms, run.plan_ms);
}

#[test]
fn each_run_adds_a_row_and_a_run_with_nothing_picked_is_recorded_too() {
    let scratch = Scratch::new("run-record-rows");
    let top = repo(&scratch, "check sh *.sh tools/ok.sh {files}\n");
    let log = recorded(&scratch);
    stage(&top, "notes.md", "x");
    run_staged(&top, Some(&log)).unwrap();
    stage(&top, "a.sh", "x");
    run_staged(&top, Some(&log)).unwrap();
    let runs = recorded_runs(&log);
    assert_eq!(runs.len(), 2);
    // Newest first.
    assert_eq!(runs[0].checks[0].outcome, Outcome::Passed);
    assert!(runs[1].passed);
    assert_eq!(runs[1].checks[0].outcome, Outcome::Skipped);
}

#[test]
fn a_crates_line_records_how_many_crates_it_was_handed() {
    let scratch = Scratch::new("run-record-crates");
    let top = repo(&scratch, "check cl *.rs,Cargo.toml tools/ok.sh {crates}\n");
    std::fs::create_dir_all(top.join("a/src")).unwrap();
    std::fs::create_dir_all(top.join("b/src")).unwrap();
    std::fs::write(top.join("Cargo.toml"), "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n").unwrap();
    for name in ["a", "b"] {
        std::fs::write(
            top.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(top.join(name).join("src/lib.rs"), "").unwrap();
    }
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-q", "-m", "Crates"]);
    stage(&top, "a/src/lib.rs", "// a\n");
    stage(&top, "b/src/lib.rs", "// b\n");
    let log = recorded(&scratch);
    run_staged(&top, Some(&log)).unwrap();
    assert_eq!(recorded_runs(&log)[0].checks[0].handed, Some((0, 2)));
}

#[test]
fn a_database_that_cannot_be_written_warns_and_leaves_the_runs_result_alone() {
    let scratch = Scratch::new("run-record-broken");
    let top = repo(&scratch, "check bad *.sh tools/fail.sh\ncheck good *.md tools/ok.sh {files}\n");
    // A folder where the database file should be: opening it fails.
    let log = recorded(&scratch);
    std::fs::create_dir(&log).unwrap();
    stage(&top, "a.md", "x");
    run_staged(&top, Some(&log)).unwrap();
    stage(&top, "a.sh", "x");
    let error = run_staged(&top, Some(&log)).unwrap_err().text();
    assert!(error.contains("bad") && !error.contains("builds.sqlite"), "{error}");
    assert!(log.is_dir());
}

fn history(scratch: &Scratch) -> (std::path::PathBuf, String) {
    let top = repo(scratch, "test t-sh scripts/*.sh tools/ok.sh\ntest t-py **/*.py tools/ok.sh\ntest t-all spec/words.md tools/ok.sh\n");
    let first = String::from_utf8(
        std::process::Command::new("git").arg("-C").arg(&top).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    std::fs::create_dir_all(top.join("scripts")).unwrap();
    std::fs::write(top.join("scripts/a.sh"), "x").unwrap();
    git(&top, &["add", "scripts/a.sh"]);
    git(&top, &["commit", "-q", "-m", "Change a script"]);
    (top, first)
}

fn names(planned: &[Planned]) -> Vec<&str> {
    planned.iter().map(|p| p.line.name.as_str()).collect()
}

#[test]
fn tests_since_picks_the_tests_whose_paths_changed() {
    let scratch = Scratch::new("tests-since");
    let (top, first) = history(&scratch);
    let lines = parse(&std::fs::read_to_string(top.join(".checks")).unwrap()).unwrap();
    let (picked, why) = select_tests(&top, &lines, &first).unwrap();
    assert_eq!(names(&picked), ["t-sh"]);
    assert_eq!(why, None);
    let (none, _) = select_tests(&top, &lines, "HEAD").unwrap();
    assert!(none.is_empty());
}

#[test]
fn tests_since_an_unknown_rev_runs_every_test_and_says_why() {
    let scratch = Scratch::new("tests-unknown");
    let (top, _) = history(&scratch);
    let lines = parse(&std::fs::read_to_string(top.join(".checks")).unwrap()).unwrap();
    let (picked, why) = select_tests(&top, &lines, "0123456789abcdef0123456789abcdef01234567").unwrap();
    assert_eq!(names(&picked), ["t-sh", "t-py", "t-all"]);
    assert!(why.unwrap().contains("not in the history"));
}
