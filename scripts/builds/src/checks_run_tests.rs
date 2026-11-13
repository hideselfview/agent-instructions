use super::*;
use crate::checks::parse;
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
    run_staged(&top).unwrap();
    assert_eq!(log(&top), "");
}

#[test]
fn a_check_gets_the_staged_files_it_matches() {
    let scratch = Scratch::new("run-files");
    let top = repo(&scratch, "check sh *.sh tools/ok.sh {files}\ncheck md *.md tools/second.sh\n");
    stage(&top, "a.sh", "x");
    stage(&top, "b.md", "x");
    run_staged(&top).unwrap();
    assert_eq!(log(&top), "ok a.sh\nsecond \n");
}

#[test]
fn a_failing_check_still_lets_the_rest_run_and_the_run_fails() {
    let scratch = Scratch::new("run-fail");
    let top = repo(&scratch, "check bad *.sh tools/fail.sh\ncheck good *.sh tools/second.sh\n");
    stage(&top, "a.sh", "x");
    let error = run_staged(&top).unwrap_err().text();
    assert_eq!(log(&top), "fail \nsecond \n");
    assert!(error.contains("bad") && !error.contains("good"), "{error}");
}

#[test]
fn a_fix_stages_again_the_files_it_changed() {
    let scratch = Scratch::new("run-fix");
    let top = repo(&scratch, "fix fmt *.txt tools/format.sh {files}\n");
    stage(&top, "a.txt", "ugly");
    run_staged(&top).unwrap();
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
    let error = run_staged(&top).unwrap_err().text();
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
    run_staged(&top).unwrap();
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
