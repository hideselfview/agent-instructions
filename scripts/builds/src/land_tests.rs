use super::*;
use crate::testing::Scratch;

/// A fixture: a bare origin, an agent clone and a detached landing clone. The
/// repo's `whole` check, a fake program, refuses a tree holding both a.txt and
/// b.txt, which two commits that each pass alone reach only together.
struct Fixture {
    scratch: Scratch,
    origin: PathBuf,
    agent: PathBuf,
    land: PathBuf,
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn identify(dir: &Path) {
    git_out(dir, &["config", "user.name", "t"]);
    git_out(dir, &["config", "user.email", "t@t"]);
    git_out(dir, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(dir: &Path, name: &str, text: &str, message: &str) -> String {
    std::fs::write(dir.join(name), text).unwrap();
    git_out(dir, &["add", name]);
    git_out(dir, &["commit", "-q", "-m", message]);
    git_out(dir, &["rev-parse", "HEAD"])
}

fn fixture(name: &str, checks: &str, compile: Option<&str>) -> Fixture {
    let scratch = Scratch::new(name);
    let root = scratch.dir.clone();
    let origin = root.join("origin.git");
    git_out(
        &root,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    let agent = root.join("agent");
    git_out(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            agent.to_str().unwrap(),
        ],
    );
    identify(&agent);
    std::fs::write(agent.join("fake-check.sh"), checks).unwrap();
    std::fs::write(
        agent.join(".checks"),
        "whole fake * sh fake-check.sh\ncheck skipped-by-land * sh not-run.sh\n",
    )
    .unwrap();
    commit_file(&agent, "base.txt", "base\n", "base");
    git_out(&agent, &["add", "fake-check.sh", ".checks"]);
    if let Some(compile) = compile {
        std::fs::write(agent.join(".land-compile"), compile).unwrap();
        git_out(&agent, &["add", ".land-compile"]);
    }
    git_out(&agent, &["commit", "-q", "--amend", "--no-edit"]);
    git_out(&agent, &["push", "-q", "origin", "HEAD:main"]);
    let land = root.join("land");
    git_out(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            land.to_str().unwrap(),
        ],
    );
    identify(&land);
    git_out(&land, &["checkout", "-q", "--detach", "origin/main"]);
    Fixture {
        scratch,
        origin,
        agent,
        land,
    }
}

/// What the stand-in `builds` answers: the project `lists project-of` prints
/// (nothing when none), the lines `design places` prints, and the command
/// ("<group> <command>") it refuses, saying `says` when that is set.
#[derive(Default)]
struct Cues {
    project: &'static str,
    places: &'static str,
    refuse: &'static str,
    says: &'static str,
    /// Keeps the background `review run` call in the commands returned.
    keep_review: bool,
}

/// Lands <args> in the fixture's landing clone with a stand-in `builds`.
/// Returns the exit status, what the landing said and the commands it ran.
fn run_land(f: &Fixture, args: &[&str], cues: Cues) -> (i32, String, Vec<String>) {
    let waits_for_review = cues.keep_review;
    let stub = f.scratch.dir.join("stub-builds");
    let log = f.scratch.dir.join("calls.log");
    let script = format!(
        "#!/bin/bash\necho \"$*\" >> '{log}'\n[ '{refuse}' = \"$1 $2\" ] && {{ echo '{says}'; exit 1; }}\ncase \"$1 $2\" in\n  'lists project-of') echo '{project}' ;;\n  'design places') printf '%s' '{places}' ;;\nesac\nexit 0\n",
        log = log.display(),
        refuse = cues.refuse,
        says = if cues.says.is_empty() {
            format!("stub refuses {}", cues.refuse)
        } else {
            cues.says.replace('\'', "'\\''")
        },
        project = cues.project,
        places = cues.places,
    );
    std::fs::write(&stub, script).unwrap();
    std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut all = vec![f.land.display().to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    let mut out = Vec::new();
    let code = match land(&stub, &all, &mut out) {
        Ok(()) => 0,
        Err(Fail::Exit(code)) => code,
        Err(Fail::Message(message)) => panic!("landing failed: {message}"),
    };
    let read = || -> Vec<String> {
        std::fs::read_to_string(&log)
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default()
    };
    // The review of a landing runs on in the background after the landing
    // returns; a landing that went through is read once its call is logged.
    let mut calls = read();
    if waits_for_review && code == 0 {
        for _ in 0..100 {
            if calls.iter().any(|call| call.starts_with("review run")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            calls = read();
        }
    }
    if !waits_for_review {
        calls.retain(|call| !call.starts_with("review run"));
    }
    // The next landing in this fixture starts a fresh log.
    let _ = std::fs::remove_file(&log);
    (code, String::from_utf8_lossy(&out).into_owned(), calls)
}

/// Makes a commit fetchable from the landing clone, as a shared object store does for worktrees.
fn publish(f: &Fixture, name: &str, sha: &str) {
    git_out(
        &f.agent,
        &["push", "-q", "origin", &format!("{sha}:refs/heads/{name}")],
    );
}

/// Commits a.txt and b.txt on the base, published for landing.
fn two_sibling_commits(f: &Fixture) -> (String, String) {
    let base = git_out(&f.agent, &["rev-parse", "HEAD"]);
    let a = commit_file(&f.agent, "a.txt", "a\n", "add a");
    git_out(&f.agent, &["checkout", "-q", "--detach", &base]);
    let b = commit_file(&f.agent, "b.txt", "b\n", "add b");
    publish(f, "a", &a);
    publish(f, "b", &b);
    (a, b)
}

fn main_of(f: &Fixture) -> String {
    git_out(&f.origin, &["rev-parse", "main"])
}

const CHECK: &str =
    "! { test -e a.txt && test -e b.txt; } || { echo 'a.txt and b.txt together'; exit 1; }\n";
const COMPILE: &str = "#!/bin/bash\n! { test -e a.txt && test -e b.txt; } || { echo 'a.txt and b.txt do not compile together'; exit 1; }\n";

fn project() -> Cues {
    Cues {
        project: "bae",
        ..Cues::default()
    }
}

#[test]
fn a_fast_forward_of_a_passing_tree_lands_and_a_cherry_pick_failing_a_check_is_refused() {
    let f = fixture("land-checks", &format!("# comment\n\n{CHECK}"), None);
    let (a, b) = two_sibling_commits(&f);
    let (code, _, _) = run_land(&f, &[&a], Cues::default());
    assert!(code == 0 && main_of(&f) == a);
    let (code, out, _) = run_land(&f, &[&b], Cues::default());
    assert!(
        code == 1 && out.contains("CHECK FAILED") && out.contains("a.txt and b.txt together"),
        "{out}"
    );
    assert_eq!(main_of(&f), a);
    assert_eq!(git_out(&f.land, &["rev-parse", "HEAD"]), a);
    assert_eq!(git_out(&f.land, &["status", "--porcelain"]), "");
}

#[test]
fn a_cherry_picked_result_passing_the_checks_is_pushed() {
    let f = fixture("land-pass", "true\n", None);
    let (a, b) = two_sibling_commits(&f);
    run_land(&f, &[&a], Cues::default());
    let (code, _, _) = run_land(&f, &[&b], Cues::default());
    assert_eq!(code, 0);
    assert_eq!(git_out(&f.origin, &["show", "main:b.txt"]), "b");
    assert_eq!(git_out(&f.origin, &["show", "main:a.txt"]), "a");
}

#[test]
fn a_fast_forward_of_a_failing_tree_is_refused_by_the_checks() {
    let f = fixture("land-ff-check", CHECK, None);
    let (a, _b) = two_sibling_commits(&f);
    git_out(&f.agent, &["checkout", "-q", "--detach", &a]);
    let c = commit_file(&f.agent, "b.txt", "b\n", "add b on a");
    publish(&f, "c", &c);
    let (code, out, _) = run_land(&f, &[&a, &c], Cues::default());
    assert!(
        code == 1 && out.contains("CHECK FAILED") && main_of(&f) != c,
        "{out}"
    );
}

#[test]
fn a_cherry_picked_result_failing_the_compile_check_is_refused() {
    let f = fixture("land-compile", "true\n", Some(COMPILE));
    let (a, b) = two_sibling_commits(&f);
    run_land(&f, &[&a], Cues::default());
    let (code, out, _) = run_land(&f, &[&b], Cues::default());
    assert!(
        code == 1
            && out.contains("COMPILE CHECK FAILED")
            && out.contains("do not compile together"),
        "{out}"
    );
    assert_eq!(main_of(&f), a);
    assert_eq!(git_out(&f.land, &["rev-parse", "HEAD"]), a);
    assert_eq!(git_out(&f.land, &["status", "--porcelain"]), "");
}

#[test]
fn a_cherry_picked_result_passing_the_compile_check_is_pushed() {
    let f = fixture("land-compile-pass", "true\n", Some("#!/bin/bash\ntrue\n"));
    let (a, b) = two_sibling_commits(&f);
    run_land(&f, &[&a], Cues::default());
    let (code, _, _) = run_land(&f, &[&b], Cues::default());
    assert_eq!(code, 0);
    assert_eq!(git_out(&f.origin, &["show", "main:b.txt"]), "b");
}

#[test]
fn a_fast_forward_runs_the_compile_check() {
    let f = fixture("land-ff-compile", "true\n", Some(COMPILE));
    let (a, _b) = two_sibling_commits(&f);
    git_out(&f.agent, &["checkout", "-q", "--detach", &a]);
    let c = commit_file(&f.agent, "b.txt", "b\n", "add b on a");
    publish(&f, "c", &c);
    let (code, out, _) = run_land(&f, &[&a, &c], Cues::default());
    assert!(
        code == 1 && out.contains("COMPILE CHECK FAILED") && main_of(&f) != c,
        "{out}"
    );
}

#[test]
fn a_repo_in_no_project_lands_with_only_the_project_lookup_and_refuses_flags() {
    let f = fixture("land-no-project", "true\n", None);
    let (a, b) = two_sibling_commits(&f);
    let (code, _, calls) = run_land(&f, &[&a], Cues::default());
    assert!(code == 0 && main_of(&f) == a);
    assert_eq!(calls, [format!("lists project-of {}", f.land.display())]);
    let (code, out, _) = run_land(&f, &["--done", "5", &b], Cues::default());
    assert!(
        code == 1 && out.contains("no project") && main_of(&f) == a,
        "{out}"
    );
}

#[test]
fn a_project_repo_names_its_work_item_before_anything_moves() {
    let f = fixture("land-project", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let land = f.land.display();
    let (code, out, calls) = run_land(&f, &[&a], project());
    assert!(
        code == 1 && out.contains("--done") && main_of(&f) != a,
        "{out}"
    );
    assert_eq!(
        calls,
        [
            format!("lists project-of {land}"),
            format!("design places {land} {a}")
        ]
    );
    let before = git_out(&f.land, &["rev-parse", "HEAD"]);
    let (code, out, _) = run_land(
        &f,
        &["--done", "7", &a],
        Cues {
            refuse: "item check-landing",
            ..project()
        },
    );
    assert!(
        code == 1 && out.contains("stub refuses") && main_of(&f) != a,
        "{out}"
    );
    assert_eq!(git_out(&f.land, &["rev-parse", "HEAD"]), before);
    let (code, _, calls) = run_land(&f, &["--done", "7", "--part", "8", &a], project());
    assert!(code == 0 && main_of(&f) == a);
    assert_eq!(
        calls.last().unwrap(),
        &format!("item landed {land} --done 7 --part 8 {a}")
    );
    assert!(!calls.iter().any(|c| c.starts_with("design landed")));
}

#[test]
fn a_cherry_picked_landing_records_the_new_commit_on_main() {
    let f = fixture("land-record-new", "true\n", None);
    let (a, b) = two_sibling_commits(&f);
    run_land(&f, &[&a], Cues::default());
    let (code, _, calls) = run_land(&f, &["--done", "7", &b], project());
    let new = main_of(&f);
    assert!(code == 0 && new != b);
    assert_eq!(
        calls.last().unwrap(),
        &format!("item landed {} --done 7 {new}", f.land.display())
    );
}

#[test]
fn design_places_need_the_design_change() {
    let f = fixture("land-design", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let land = f.land.display();
    let place = Cues {
        places: "place design/mocks/button",
        ..project()
    };
    let (code, out, _) = run_land(
        &f,
        &["--done", "7", &a],
        Cues {
            places: "place design/mocks/button",
            ..project()
        },
    );
    assert!(
        code == 1 && out.contains("--design") && main_of(&f) != a,
        "{out}"
    );
    let (code, out, _) = run_land(
        &f,
        &["--done", "7", &a],
        Cues {
            places: "shared design/system/tokens",
            ..project()
        },
    );
    assert!(
        code == 1 && out.contains("--design") && main_of(&f) != a,
        "{out}"
    );
    let (code, out, _) = run_land(&f, &["--done", "7", "--design", "3", &a], project());
    assert!(code == 1 && out.contains("no design place"), "{out}");
    let (code, _, _) = run_land(
        &f,
        &["--done", "7", "--design", "3", &a],
        Cues {
            refuse: "design check",
            ..place
        },
    );
    assert!(code == 1 && main_of(&f) != a);
    let (code, _, calls) = run_land(
        &f,
        &["--done", "7", "--design", "3", &a],
        Cues {
            places: "place design/mocks/button",
            ..project()
        },
    );
    assert_eq!(code, 0);
    assert!(calls.contains(&format!("design check 3 {land} {a}")));
    assert_eq!(
        calls[calls.len() - 2..],
        [
            format!("item landed {land} --done 7 {a}"),
            format!("design landed 3 {land} {a}")
        ]
    );
}

#[test]
fn shared_only_design_files_with_the_design_change_land() {
    let f = fixture("land-shared", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let (code, _, calls) = run_land(
        &f,
        &["--done", "7", "--design", "3", &a],
        Cues {
            places: "shared design/system/tokens",
            ..project()
        },
    );
    assert_eq!(code, 0);
    assert_eq!(
        calls.last().unwrap(),
        &format!("design landed 3 {} {a}", f.land.display())
    );
}

#[test]
fn a_design_change_alone_lands_commits_that_are_all_design_files() {
    let f = fixture("land-design-alone", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let land = f.land.display();
    let (code, out, _) = run_land(&f, &["--design", "3", &a], project());
    assert!(
        code == 1 && out.contains("no design place") && main_of(&f) != a,
        "{out}"
    );
    let (code, out, _) = run_land(
        &f,
        &[&a],
        Cues {
            places: "place design/mocks/button",
            ..project()
        },
    );
    assert!(
        code == 1 && out.contains("--done") && main_of(&f) != a,
        "{out}"
    );
    let (code, out, calls) = run_land(
        &f,
        &["--design", "3", &a],
        Cues {
            places: "place design/mocks/button",
            refuse: "design only",
            says: "the commits also change files that are no design file: src/x.rs",
            ..project()
        },
    );
    assert!(
        code == 1 && out.contains("src/x.rs") && out.contains("--done") && main_of(&f) != a,
        "{out}"
    );
    assert!(!calls.iter().any(|c| c.starts_with("design check")));
    let (code, out, _) = run_land(
        &f,
        &["--design", "3", &a],
        Cues {
            places: "place design/mocks/button",
            refuse: "design check",
            says: "design change 3 waits on the person's review",
            ..project()
        },
    );
    assert!(
        code == 1 && out.contains("waits on the person's review") && main_of(&f) != a,
        "{out}"
    );
    let (code, _, calls) = run_land(
        &f,
        &["--design", "3", &a],
        Cues {
            places: "place design/mocks/button\nspec spec/bae.md#Sync",
            ..project()
        },
    );
    assert!(code == 0 && main_of(&f) == a);
    assert!(calls.contains(&format!("design only {land} {a}")));
    assert!(calls.contains(&format!("design check 3 {land} {a}")));
    assert_eq!(
        calls.last().unwrap(),
        &format!("design landed 3 {land} {a}")
    );
    assert!(!calls.iter().any(|c| c.starts_with("item ")));
}

#[test]
fn a_failed_design_record_after_a_design_only_push_exits_with_the_rerun() {
    let f = fixture("land-design-record", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let (code, out, _) = run_land(
        &f,
        &["--design", "3", &a],
        Cues {
            places: "place design/mocks/button",
            refuse: "design landed",
            ..project()
        },
    );
    assert!(
        code == 1 && main_of(&f) == a && out.contains("LANDED, NOT RECORDED:"),
        "{out}"
    );
    assert!(out.contains(&format!(
        "rerun: builds design landed 3 {} {a}",
        f.land.display()
    )));
    assert!(!out.contains("item landed"));
}

#[test]
fn a_failed_record_exits_with_the_rerun_and_the_rerun_records_it() {
    let f = fixture("land-record", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let (code, out, _) = run_land(
        &f,
        &["--done", "7", &a],
        Cues {
            refuse: "item landed",
            ..project()
        },
    );
    assert!(
        code == 1 && main_of(&f) == a && out.contains("LANDED, NOT RECORDED:"),
        "{out}"
    );
    assert!(out.contains(&format!(
        "rerun: builds item landed {} --done 7 {a}",
        f.land.display()
    )));
    let (code, _, calls) = run_land(&f, &["--done", "7", &a], project());
    assert_eq!(code, 0);
    assert_eq!(
        calls.last().unwrap(),
        &format!("item landed {} --done 7 {a}", f.land.display())
    );
}

#[test]
fn a_worktree_on_a_branch_is_refused() {
    let f = fixture("land-branch", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    git_out(&f.land, &["checkout", "-q", "-b", "work"]);
    let (code, out, _) = run_land(&f, &[&a], Cues::default());
    assert!(
        code == 1 && out.contains("is on branch work; land from the detached landing worktree"),
        "{out}"
    );
    assert_ne!(main_of(&f), a);
}

#[test]
fn unknown_options_and_missing_numbers_are_refused() {
    let f = fixture("land-options", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let (code, out, _) = run_land(&f, &["--bogus", &a], Cues::default());
    assert!(
        code == 1 && out.contains("refusing: unknown option --bogus"),
        "{out}"
    );
    let (code, out, _) = run_land(&f, &["--done"], Cues::default());
    assert!(
        code == 1 && out.contains("--done needs an item number"),
        "{out}"
    );
    let (code, out, _) = run_land(&f, &["--design", "1", "--design", "2", &a], Cues::default());
    assert!(code == 1 && out.contains("--design given twice"), "{out}");
    let (code, out, _) = run_land(&f, &[], Cues::default());
    assert!(code == 1 && out.contains("usage: builds land"), "{out}");
}

#[test]
fn landing_a_commit_already_on_main_says_so_and_records_its_copy() {
    let f = fixture("land-already", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    run_land(&f, &[&a], Cues::default());
    let (code, out, calls) = run_land(&f, &["--done", "7", &a], project());
    assert!(
        code == 0 && out.contains("already on origin/main:"),
        "{out}"
    );
    assert_eq!(
        calls.last().unwrap(),
        &format!("item landed {} --done 7 {a}", f.land.display())
    );
}

#[test]
fn a_landing_runs_only_the_whole_lines_and_says_each_ones_time() {
    // not-run.sh does not exist: a plain check line would fail if land ran it.
    let f = fixture("land-whole-only", "true\n", None);
    let (a, b) = two_sibling_commits(&f);
    run_land(&f, &[&a], Cues::default());
    let (code, out, _) = run_land(&f, &[&b], Cues::default());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("whole fake: ok, "), "{out}");
    assert!(!out.contains("skipped-by-land"), "{out}");
}

#[test]
fn every_failing_whole_line_is_named_with_its_output() {
    let f = fixture("land-whole-many", CHECK, None);
    std::fs::write(
        f.agent.join(".checks"),
        "whole first * sh fake-check.sh\nwhole second * sh fake-check.sh\n",
    )
    .unwrap();
    git_out(&f.agent, &["commit", "-q", "-a", "-m", "two checks"]);
    git_out(&f.agent, &["push", "-q", "origin", "HEAD:main"]);
    let (a, b) = two_sibling_commits(&f);
    run_land(&f, &[&a], Cues::default());
    let (code, out, _) = run_land(&f, &[&b], Cues::default());
    assert!(
        code == 1
            && out.contains("nothing pushed: first, second")
            && out.contains("--- first")
            && out.contains("--- second")
            && out.contains("whole first: FAILED, "),
        "{out}"
    );
}

#[test]
fn a_leftover_land_checks_file_refuses_the_landing() {
    let f = fixture("land-old-file", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    std::fs::write(f.agent.join(".land-checks"), "true\n").unwrap();
    git_out(&f.agent, &["add", ".land-checks"]);
    git_out(&f.agent, &["commit", "-q", "-m", "old file"]);
    let old = git_out(&f.agent, &["rev-parse", "HEAD"]);
    git_out(&f.agent, &["push", "-q", "origin", "HEAD:main"]);
    let (code, out, _) = run_land(&f, &[&a], Cues::default());
    assert!(
        code == 1 && out.contains(".land-checks") && out.contains("`whole` line"),
        "{out}"
    );
    assert_eq!(main_of(&f), old);
}

#[test]
fn a_review_that_fails_does_not_fail_the_landing() {
    let f = fixture("land-review-fails", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let (code, out, calls) = run_land(
        &f,
        &["--done", "7", &a],
        Cues {
            refuse: "review run",
            says: "1 blocking finding",
            keep_review: true,
            ..project()
        },
    );
    assert!(code == 0 && main_of(&f) == a, "{out}");
    assert!(calls.iter().any(|c| c.starts_with("item landed")));
    assert!(calls.iter().any(|c| c.starts_with("review run")));
}

#[test]
fn the_landed_commits_are_reviewed_in_the_background_after_the_push_and_the_record() {
    let f = fixture("land-review-record-only", "true\n", None);
    let (a, _b) = two_sibling_commits(&f);
    let land = f.land.display();
    let before = main_of(&f);
    let (code, _, calls) = run_land(
        &f,
        &["--done", "7", "--part", "3", &a],
        Cues {
            keep_review: true,
            ..project()
        },
    );
    assert!(code == 0 && main_of(&f) == a);
    let review = calls
        .iter()
        .position(|c| c.starts_with("review run"))
        .expect("the landed commits are reviewed");
    assert_eq!(
        calls[review],
        format!(
            "review run {land} {before} --head {a} --landing items:3,7 --project bae --record-only --item 7 --item 3"
        )
    );
    assert!(
        calls
            .iter()
            .position(|c| c.starts_with("item landed"))
            .unwrap()
            < review
    );
}

#[test]
fn a_failing_check_stops_the_landing_before_any_review_and_a_repo_in_no_project_has_none() {
    let f = fixture("land-review-order", CHECK, None);
    let (a, b) = two_sibling_commits(&f);
    run_land(&f, &["--done", "7", &a], project());
    let (code, _, calls) = run_land(&f, &["--done", "8", &b], project());
    assert_eq!(code, 1);
    assert!(!calls.iter().any(|c| c.starts_with("review run")));
    let g = fixture("land-review-none", "true\n", None);
    let (a, _b) = two_sibling_commits(&g);
    let (code, _, calls) = run_land(&g, &[&a], Cues::default());
    assert_eq!(code, 0);
    assert!(!calls.iter().any(|c| c.starts_with("review")));
}
