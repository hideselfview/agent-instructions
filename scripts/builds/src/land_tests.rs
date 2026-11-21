use super::*;
use crate::testing::{Scratch, git_out};

/// The landing in <f>, which records its run in the store of its project in
/// <f>'s places, when there is one.
fn land(f: &Fixture, program: &Path, args: &[String], out: &mut dyn Write) -> Result<()> {
    land_recorded(&f.scratch.places(), program, args, out, None)
}

/// A fixture: a bare origin and the repo's main checkout, cloned from it,
/// whose side worktrees the tests add. The repo's `whole` check, a fake
/// program, refuses a tree holding both a.txt and b.txt, which two branches
/// that each pass alone reach only together.
struct Fixture {
    scratch: Scratch,
    origin: PathBuf,
    main: PathBuf,
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

/// The fixture, with <checks> as the fake check's script, `{root}` in it
/// standing for the scratch folder.
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
    let main = root.join("repo");
    git_out(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            main.to_str().unwrap(),
        ],
    );
    identify(&main);
    let checks = checks.replace("{root}", &root.display().to_string());
    std::fs::write(main.join("fake-check.sh"), checks).unwrap();
    std::fs::write(
        main.join(".checks"),
        "whole fake * sh fake-check.sh\ncheck skipped-by-land * sh not-run.sh\n",
    )
    .unwrap();
    commit_file(&main, "base.txt", "base\n", "base");
    git_out(&main, &["add", "fake-check.sh", ".checks"]);
    if let Some(compile) = compile {
        std::fs::write(main.join(".land-compile"), compile).unwrap();
        git_out(&main, &["add", ".land-compile"]);
    }
    git_out(&main, &["commit", "-q", "--amend", "--no-edit"]);
    git_out(&main, &["push", "-q", "origin", "HEAD:main"]);
    Fixture {
        scratch,
        origin,
        main,
    }
}

impl Fixture {
    /// Side worktree <n> of the main checkout, on a new branch <branch> from
    /// origin/main, claimed for its agent.
    fn side(&self, n: usize, branch: &str) -> PathBuf {
        let dir = self.scratch.dir.join(format!("repo-worktrees/side-{n}"));
        git_out(
            &self.main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                dir.to_str().unwrap(),
                "origin/main",
            ],
        );
        claim(&dir);
        dir
    }
}

/// Claims <dir> for its agent, as `builds claim` does.
fn claim(dir: &Path) {
    let checkout = crate::checkout::Checkout::at(dir).unwrap().unwrap();
    crate::claim::Claim {
        who: "agent A".into(),
        since: 1_790_000_000,
        note: None,
    }
    .write(&checkout.claim_file())
    .unwrap();
}

/// What the stand-in `builds` answers: the project `lists project-of` prints
/// (nothing when none), the lines `design places` prints, and the command
/// ("<group> <command>") it refuses, saying `says` when that is set. `title`
/// is what `design title` prints, `landing_build` the shell words that
/// answer `app-build --landing-build` (its result is $5) and `app_build` those
/// that answer `app-build <repo>`, `{root}` in either standing for the scratch
/// folder. Each app build that is not `--landing-build` also logs the folder
/// it ran in.
#[derive(Default)]
struct Cues {
    project: &'static str,
    title: &'static str,
    places: &'static str,
    refuse: &'static str,
    says: &'static str,
    landing_build: &'static str,
    app_build: &'static str,
}

/// Writes the stand-in `builds` for <cues> and returns it with the file it
/// logs each command to.
fn stub(f: &Fixture, cues: &Cues) -> (PathBuf, PathBuf) {
    let or_true = |words: &'static str| if words.is_empty() { "true" } else { words };
    let root = f.scratch.dir.display().to_string();
    let stub = f.scratch.dir.join("stub-builds");
    let log = f.scratch.dir.join("calls.log");
    let script = format!(
        "#!/bin/bash\necho \"$*\" >> '{log}'\n[ \"$1\" = app-build ] && [ \"$2\" != --landing-build ] && echo \"app-build ran in $PWD\" >> '{log}'\n[ '{refuse}' = \"$1 $2\" ] && {{ echo '{says}'; exit 1; }}\ncase \"$1 $2\" in\n  'lists project-of') echo '{project}' ;;\n  'design places') printf '%s' '{places}' ;;\n  'design title') echo '{title}' ;;\n  'app-build --landing-build') {landing_build} ;;\n  'app-build repo') {app_build} ;;\nesac\nexit 0\n",
        log = log.display(),
        refuse = cues.refuse,
        says = if cues.says.is_empty() {
            format!("stub refuses {}", cues.refuse)
        } else {
            cues.says.replace('\'', "'\\''")
        },
        project = cues.project,
        title = cues.title,
        places = cues.places,
        landing_build = or_true(cues.landing_build).replace("{root}", &root),
        app_build = or_true(cues.app_build).replace("{root}", &root),
    );
    std::fs::write(&stub, script).unwrap();
    std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    (stub, log)
}

/// Lands <args> in the worktree <dir> with a stand-in `builds`. Returns the
/// exit status, what the landing said and the commands it ran.
fn run_land(f: &Fixture, dir: &Path, args: &[&str], cues: Cues) -> (i32, String, Vec<String>) {
    let (stub, log) = stub(f, &cues);
    let mut all = vec![dir.display().to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    let mut out = Vec::new();
    let code = match land(f, &stub, &all, &mut out) {
        Ok(()) => 0,
        Err(Fail::Exit(code)) => code,
        Err(Fail::Message(message)) => panic!("landing failed: {message}"),
    };
    let calls: Vec<String> = std::fs::read_to_string(&log)
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default();
    // The next landing in this fixture starts a fresh log.
    let _ = std::fs::remove_file(&log);
    (code, String::from_utf8_lossy(&out).into_owned(), calls)
}

/// <dir>'s landing of <args> with the stand-in `builds`, and the record land
/// made of the run.
fn profiled(f: &Fixture, dir: &Path, args: &[&str], cues: Cues) -> (i32, Option<LandRun>) {
    let (stub, _) = stub(f, &cues);
    let mut all = vec![dir.display().to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    let mut out = Vec::new();
    let Ended::Ran { result, run, .. } =
        land_profiled(&stub, &all, &mut out, StoreUse::Held { _holding: None })
    else {
        panic!("no install ran in the test");
    };
    let code = match result {
        Ok(()) => 0,
        Err(Fail::Exit(code)) => code,
        Err(Fail::Message(message)) => panic!("landing failed: {message}"),
    };
    (code, run.map(|run| *run))
}

/// Side-1 on branch `a` with a.txt committed, and side-2 on branch `b` with
/// b.txt, both from the base.
fn two_branches(f: &Fixture) -> (PathBuf, String, PathBuf, String) {
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let two = f.side(2, "b");
    let b = commit_file(&two, "b.txt", "b\n", "add b");
    (one, a, two, b)
}

fn main_of(f: &Fixture) -> String {
    git_out(&f.origin, &["rev-parse", "main"])
}

/// The branch <dir> is on, or None when its HEAD is detached.
fn branch_of(dir: &Path) -> Option<String> {
    git_text(dir, &["symbolic-ref", "-q", "--short", "HEAD"])
}

fn head_of(dir: &Path) -> String {
    git_out(dir, &["rev-parse", "HEAD"])
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
    let (one, a, two, b) = two_branches(&f);
    let (code, _, _) = run_land(&f, &one, &[&a], Cues::default());
    assert!(code == 0 && main_of(&f) == a);
    let (code, out, _) = run_land(&f, &two, &[&b], Cues::default());
    assert!(
        code == 1 && out.contains("CHECK FAILED") && out.contains("a.txt and b.txt together"),
        "{out}"
    );
    assert_eq!(main_of(&f), a);
    assert_eq!(
        branch_of(&two).as_deref(),
        Some("b"),
        "the refused landing checks the branch out again"
    );
    assert_eq!(head_of(&two), b);
    assert_eq!(git_out(&two, &["status", "--porcelain"]), "");
}

#[test]
fn a_cherry_pick_that_conflicts_names_the_files_says_what_to_do_and_exits_1() {
    let f = fixture("land-conflict", "true\n", None);
    let one = f.side(1, "a");
    let a = commit_file(&one, "shared.txt", "from a\n", "a writes shared");
    let two = f.side(2, "b");
    let b = commit_file(&two, "shared.txt", "from b\n", "b writes shared");
    let (code, _, _) = run_land(&f, &one, &[&a], Cues::default());
    assert!(code == 0 && main_of(&f) == a);
    let (code, out, _) = run_land(&f, &two, &[&b], Cues::default());
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("CONFLICT") && out.contains("\n  shared.txt"),
        "{out}"
    );
    assert!(
        out.contains("Nothing landed") && out.contains("git rebase origin/main"),
        "{out}"
    );
    assert_eq!(main_of(&f), a);
    assert_eq!(branch_of(&two).as_deref(), Some("b"));
    assert_eq!(head_of(&two), b);
    assert_eq!(git_out(&two, &["status", "--porcelain"]), "");
}

#[test]
fn a_refused_landing_whose_reset_fails_stays_on_the_result_and_says_so() {
    // The check takes the worktree's index lock and fails, so the reset after
    // the refusal fails too.
    let f = fixture(
        "land-reset-fails",
        "touch \"$(git rev-parse --git-dir)/index.lock\"; exit 1\n",
        None,
    );
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let (code, out, _) = run_land(&f, &one, &[&a], Cues::default());
    assert!(
        code == 1 && out.contains("could not reset") && out.contains("branch a by hand"),
        "{out}"
    );
    assert_eq!(
        branch_of(&one),
        None,
        "land runs no checkout after the failed reset"
    );
}

#[test]
fn a_cherry_picked_result_passing_the_checks_is_pushed() {
    let f = fixture("land-pass", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    run_land(&f, &one, &[&a], Cues::default());
    let (code, _, _) = run_land(&f, &two, &[&b], Cues::default());
    assert_eq!(code, 0);
    assert_eq!(git_out(&f.origin, &["show", "main:b.txt"]), "b");
    assert_eq!(git_out(&f.origin, &["show", "main:a.txt"]), "a");
}

#[test]
fn a_fast_forward_of_a_failing_tree_is_refused_by_the_checks() {
    let f = fixture("land-ff-check", CHECK, None);
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let c = commit_file(&one, "b.txt", "b\n", "add b on a");
    let (code, out, _) = run_land(&f, &one, &[&a, &c], Cues::default());
    assert!(
        code == 1 && out.contains("CHECK FAILED") && main_of(&f) != c,
        "{out}"
    );
}

#[test]
fn a_cherry_picked_result_failing_the_compile_check_is_refused() {
    let f = fixture("land-compile", "true\n", Some(COMPILE));
    let (one, a, two, b) = two_branches(&f);
    run_land(&f, &one, &[&a], Cues::default());
    let (code, out, _) = run_land(&f, &two, &[&b], Cues::default());
    assert!(
        code == 1
            && out.contains("COMPILE CHECK FAILED")
            && out.contains("do not compile together"),
        "{out}"
    );
    assert_eq!(main_of(&f), a);
    assert_eq!(branch_of(&two).as_deref(), Some("b"));
    assert_eq!(head_of(&two), b);
    assert_eq!(git_out(&two, &["status", "--porcelain"]), "");
}

#[test]
fn a_cherry_picked_result_passing_the_compile_check_is_pushed() {
    let f = fixture("land-compile-pass", "true\n", Some("#!/bin/bash\ntrue\n"));
    let (one, a, two, b) = two_branches(&f);
    run_land(&f, &one, &[&a], Cues::default());
    let (code, _, _) = run_land(&f, &two, &[&b], Cues::default());
    assert_eq!(code, 0);
    assert_eq!(git_out(&f.origin, &["show", "main:b.txt"]), "b");
}

#[test]
fn a_fast_forward_runs_the_compile_check() {
    let f = fixture("land-ff-compile", "true\n", Some(COMPILE));
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let c = commit_file(&one, "b.txt", "b\n", "add b on a");
    let (code, out, _) = run_land(&f, &one, &[&a, &c], Cues::default());
    assert!(
        code == 1 && out.contains("COMPILE CHECK FAILED") && main_of(&f) != c,
        "{out}"
    );
}

#[test]
fn a_repo_in_no_project_lands_with_only_the_project_lookup_and_the_release_and_refuses_flags() {
    let f = fixture("land-no-project", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    let (code, _, calls) = run_land(&f, &one, &[&a], Cues::default());
    assert!(code == 0 && main_of(&f) == a);
    assert_eq!(
        calls,
        [
            format!("lists project-of {}", one.display()),
            format!("release {}", one.display())
        ]
    );
    let (code, out, _) = run_land(&f, &two, &["--done", "5", &b], Cues::default());
    assert!(
        code == 1 && out.contains("no project") && main_of(&f) == a,
        "{out}"
    );
}

#[test]
fn done_refuses_while_commits_of_the_branch_stay_unlanded() {
    let f = fixture("done-leaves-commits", CHECK, None);
    let one = f.side(1, "a");
    let first = commit_file(&one, "a.txt", "a\n", "add a");
    let second = commit_file(&one, "c.txt", "c\n", "add c");
    let (code, out, calls) = run_land(&f, &one, &["--done", "7", &first], project());
    assert!(
        code == 1
            && out.contains("--part")
            && out.contains(&format!("{} add c", &second[..7]))
            && main_of(&f) != first,
        "{out}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("item landed")),
        "{calls:?}"
    );
    assert_eq!(branch_of(&one).as_deref(), Some("a"));
    let (code, _, calls) = run_land(&f, &one, &["--part", "7", &first], project());
    assert!(code == 0 && main_of(&f) == first);
    assert!(calls.iter().any(|c| c.starts_with("item landed")));
    let (code, _, _) = run_land(&f, &one, &["--done", "7", &second], project());
    assert!(
        code == 0 && main_of(&f) != first,
        "the last commit closes the item"
    );
}

#[test]
fn a_project_repo_names_its_work_item_before_anything_moves() {
    let f = fixture("land-project", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let dir = one.display();
    let (code, out, calls) = run_land(&f, &one, &[&a], project());
    assert!(
        code == 1 && out.contains("--done") && main_of(&f) != a,
        "{out}"
    );
    assert_eq!(
        calls,
        [
            format!("lists project-of {dir}"),
            format!("design places {dir} {a} --project bae")
        ]
    );
    let (code, out, _) = run_land(
        &f,
        &one,
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
    assert_eq!(branch_of(&one).as_deref(), Some("a"));
    let (code, _, calls) = run_land(&f, &one, &["--done", "7", "--part", "8", &a], project());
    assert!(code == 0 && main_of(&f) == a);
    assert_eq!(
        calls[calls.len() - 2..],
        [
            format!("item landed {dir} --done 7 --part 8 {a} --project bae --by agent A"),
            format!("review landed {dir} --project bae --landing branch:a {a} --by agent A")
        ],
        "every record names the worktree's claim; a --part landing keeps the claim"
    );
    assert!(!calls.iter().any(|c| c.starts_with("design landed")));
}

#[test]
fn an_unclaimed_worktree_of_a_project_is_refused_before_anything_moves() {
    let f = fixture("land-unclaimed", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    let checkout = crate::checkout::Checkout::at(&one).unwrap().unwrap();
    std::fs::remove_file(checkout.claim_file()).unwrap();
    let (code, out, calls) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(
        code == 1 && out.contains("is not claimed") && main_of(&f) != a,
        "{out}"
    );
    assert_eq!(head_of(&one), a);
    assert_eq!(calls, [format!("lists project-of {}", one.display())]);
    // A repo in no project records nothing, so land takes the unclaimed
    // worktree and has no claim to release.
    let (code, out, calls) = run_land(&f, &one, &[&a], Cues::default());
    assert!(code == 0 && main_of(&f) == a, "{out}");
    assert!(!calls.iter().any(|c| c.starts_with("release")), "{calls:?}");
    let _ = (two, b);
}

#[test]
fn a_cherry_picked_landing_records_the_new_commit_on_main_and_releases_the_worktree() {
    let f = fixture("land-record-new", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    run_land(&f, &one, &[&a], Cues::default());
    let (code, out, calls) = run_land(&f, &two, &["--done", "7", &b], project());
    let new = main_of(&f);
    assert!(code == 0 && new != b);
    let dir = two.display();
    assert_eq!(
        calls[calls.len() - 3..],
        [
            format!("item landed {dir} --done 7 {new} --project bae --by agent A"),
            format!("review landed {dir} --project bae --landing branch:b {b} --by agent A"),
            format!("release {dir}")
        ],
        "the items record the commit on main; the review the commit it read"
    );
    assert_eq!(branch_of(&two), None, "{out}");
    assert_eq!(
        head_of(&two),
        new,
        "the landing leaves the worktree detached at the landed commit"
    );
    assert!(out.contains(&format!("released {dir}")), "{out}");
}

#[test]
fn a_landing_that_leaves_commits_on_the_branch_goes_back_on_it_and_keeps_the_claim() {
    let f = fixture("land-leaves-work", "true\n", None);
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let later = commit_file(&one, "d.txt", "d\n", "add d");
    let (code, out, calls) = run_land(&f, &one, &["--part", "7", &a], project());
    assert!(code == 0 && main_of(&f) == a, "{out}");
    assert_eq!(branch_of(&one).as_deref(), Some("a"));
    assert_eq!(head_of(&one), later);
    assert!(out.contains("1 commit(s) there wait to land"), "{out}");
    assert!(!calls.iter().any(|c| c.starts_with("release")), "{calls:?}");
    let dir = one.display();
    assert!(
        calls.contains(&format!(
            "review landed {dir} --project bae --landing branch:a {a} --more-to-land --by agent A"
        )),
        "the review's landing stays open for the commit that waits: {calls:?}"
    );
}

#[test]
fn a_part_landing_keeps_the_claim() {
    let f = fixture("land-part", "true\n", None);
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let (code, out, calls) = run_land(&f, &one, &["--part", "7", &a], project());
    assert!(code == 0 && main_of(&f) == a, "{out}");
    assert_eq!(branch_of(&one), None);
    assert!(!calls.iter().any(|c| c.starts_with("release")), "{calls:?}");
}

#[test]
fn a_refused_release_keeps_the_claim_and_fails_nothing() {
    let f = fixture("land-release-refused", "true\n", None);
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    // The stand-in refuses "<group> <command>"; `release DIR` has no command
    // word, so a wrapper refuses it.
    let (stub, _) = stub(&f, &project());
    let refusing = f.scratch.dir.join("stub-refuses-release");
    std::fs::write(
        &refusing,
        format!(
            "#!/bin/bash\n[ \"$1\" = release ] && {{ echo 'still runs item 9'; exit 1; }}\nexec '{}' \"$@\"\n",
            stub.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &refusing,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let mut out = Vec::new();
    let args = vec![
        one.display().to_string(),
        "--done".into(),
        "7".into(),
        a.clone(),
    ];
    let result = land(&f, &refusing, &args, &mut out);
    let out = String::from_utf8_lossy(&out);
    assert!(result.is_ok() && main_of(&f) == a, "{out}");
    assert!(
        out.contains(&format!(
            "kept the claim of {}: still runs item 9",
            one.display()
        )),
        "{out}"
    );
}

#[test]
fn design_places_need_the_design_change() {
    let f = fixture("land-design", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let dir = one.display();
    let place = Cues {
        places: "place design/mocks/button",
        ..project()
    };
    let (code, out, _) = run_land(
        &f,
        &one,
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
        &one,
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
    let (code, out, _) = run_land(&f, &one, &["--done", "7", "--design", "3", &a], project());
    assert!(code == 1 && out.contains("no design place"), "{out}");
    let (code, _, _) = run_land(
        &f,
        &one,
        &["--done", "7", "--design", "3", &a],
        Cues {
            refuse: "design check",
            ..place
        },
    );
    assert!(code == 1 && main_of(&f) != a);
    let (code, _, calls) = run_land(
        &f,
        &one,
        &["--done", "7", "--design", "3", &a],
        Cues {
            places: "place design/mocks/button",
            ..project()
        },
    );
    assert_eq!(code, 0);
    assert!(calls.contains(&format!("design check 3 {dir} {a} --project bae")));
    assert_eq!(
        calls[calls.len() - 4..],
        [
            format!("item landed {dir} --done 7 {a} --project bae --by agent A"),
            format!("design landed 3 {dir} {a} --project bae --by agent A"),
            format!("review landed {dir} --project bae --landing branch:a {a} --by agent A"),
            format!("release {dir}")
        ]
    );
}

#[test]
fn shared_only_design_files_with_the_design_change_land() {
    let f = fixture("land-shared", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, _, calls) = run_land(
        &f,
        &one,
        &["--done", "7", "--design", "3", &a],
        Cues {
            places: "shared design/system/tokens",
            ..project()
        },
    );
    assert_eq!(code, 0);
    assert!(calls.contains(&format!(
        "design landed 3 {} {a} --project bae --by agent A",
        one.display()
    )));
}

#[test]
fn a_design_change_alone_lands_commits_that_are_all_design_files() {
    let f = fixture("land-design-alone", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let dir = one.display();
    let (code, out, _) = run_land(&f, &one, &["--design", "3", &a], project());
    assert!(
        code == 1 && out.contains("no design place") && main_of(&f) != a,
        "{out}"
    );
    let (code, out, _) = run_land(
        &f,
        &one,
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
        &one,
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
        &one,
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
        &one,
        &["--design", "3", &a],
        Cues {
            places: "place design/mocks/button\nspec spec/bae.md#Sync",
            ..project()
        },
    );
    assert!(code == 0 && main_of(&f) == a);
    assert!(calls.contains(&format!("design only {dir} {a} --project bae")));
    assert!(calls.contains(&format!("design check 3 {dir} {a} --project bae")));
    assert!(calls.contains(&format!(
        "review check-landing {dir} --project bae --landing branch:a {a}"
    )));
    assert_eq!(
        calls[calls.len() - 3..],
        [
            format!("design landed 3 {dir} {a} --project bae --by agent A"),
            format!("review landed {dir} --project bae --landing branch:a {a} --by agent A"),
            format!("release {dir}")
        ],
        "a design landing releases the worktree too"
    );
    assert!(!calls.iter().any(|c| c.starts_with("item ")));
}

#[test]
fn several_design_changes_land_in_one_call_and_each_is_recorded_as_landed() {
    let f = fixture("land-designs", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let dir = one.display();
    let places = "place design/mocks/button\nplace design/mocks/row";
    let (code, out, calls) = run_land(
        &f,
        &one,
        &["--design", "3", "--design", "4", &a],
        Cues {
            places,
            refuse: "design check",
            says: "design changes #3, #4 do not resolve design/mocks/row",
            ..project()
        },
    );
    assert!(
        code == 1 && out.contains("do not resolve design/mocks/row") && main_of(&f) != a,
        "{out}"
    );
    assert!(
        calls.contains(&format!("design check 3,4 {dir} {a} --project bae")),
        "one check decides whether the changes together resolve every place: {calls:?}"
    );
    let (code, out, calls) = run_land(
        &f,
        &one,
        &["--design", "3", "--design", "4", &a],
        Cues {
            places,
            ..project()
        },
    );
    assert!(code == 0 && main_of(&f) == a, "{out}");
    assert!(calls.contains(&format!("design only {dir} {a} --project bae")));
    assert!(calls.contains(&format!("design check 3,4 {dir} {a} --project bae")));
    assert_eq!(
        calls[calls.len() - 4..],
        [
            format!("design landed 3 {dir} {a} --project bae --by agent A"),
            format!("design landed 4 {dir} {a} --project bae --by agent A"),
            format!("review landed {dir} --project bae --landing branch:a {a} --by agent A"),
            format!("release {dir}")
        ]
    );
}

#[test]
fn a_failed_design_record_after_a_design_only_push_exits_with_the_rerun() {
    let f = fixture("land-design-record", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, out, calls) = run_land(
        &f,
        &one,
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
        "rerun: builds design landed 3 {} {a} --project bae --by 'agent A'",
        one.display()
    )));
    assert!(!out.contains("item landed"));
    assert!(
        !calls.iter().any(|c| c.starts_with("release")),
        "a landing whose records failed keeps the claim: {calls:?}"
    );
}

#[test]
fn a_failed_record_exits_with_the_rerun_and_the_rerun_records_it() {
    let f = fixture("land-record", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, out, _) = run_land(
        &f,
        &one,
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
        "rerun: builds item landed {} --done 7 {a} --project bae --by 'agent A'",
        one.display()
    )));
    let (code, _, calls) = run_land(&f, &one, &["--done", "7", &a], project());
    assert_eq!(code, 0);
    assert!(calls.contains(&format!(
        "item landed {} --done 7 {a} --project bae --by agent A",
        one.display()
    )));
}

#[test]
fn a_commit_not_on_the_worktrees_head_is_refused() {
    let f = fixture("land-other-branch", "true\n", None);
    let (one, _a, _two, b) = two_branches(&f);
    let (code, out, calls) = run_land(&f, &one, &[&b], Cues::default());
    assert!(
        code == 1
            && out.contains(&format!("{b} is not on branch a in {}", one.display()))
            && out.contains(
                "run builds land in the worktree that has the commit's branch checked out"
            ),
        "{out}"
    );
    assert!(calls.is_empty(), "{calls:?}");
    assert_ne!(main_of(&f), b);
}

#[test]
fn short_ids_land_and_are_recorded_by_their_full_ids() {
    let f = fixture("land-short-ids", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let short = &a[..10];
    let (code, run) = profiled(&f, &one, &["--done", "7", short], project());
    assert_eq!(code, 0);
    let run = run.expect("a repo in a project records its landing");
    assert!(run.landed);
    assert_eq!(run.commits.len(), 1);
    assert_eq!(run.commits[0].0, a);
    assert_eq!(run.commits[0].0.len(), 40);
}

#[test]
fn an_argument_that_names_no_commit_is_refused_before_anything_moves() {
    let f = fixture("land-no-such-commit", "true\n", None);
    let (one, _a, _two, _b) = two_branches(&f);
    let before = main_of(&f);
    let (code, out, calls) = run_land(&f, &one, &["0123456789abcdef"], Cues::default());
    assert!(
        code == 1 && out.contains("0123456789abcdef does not name exactly one commit"),
        "{out}"
    );
    assert!(calls.is_empty(), "{calls:?}");
    assert_eq!(main_of(&f), before);
}

#[test]
fn an_option_after_the_commits_is_refused_by_name() {
    let f = fixture("land-late-option", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let before = main_of(&f);
    let (code, out, calls) = run_land(&f, &one, &[&a[..8], "--project", "builds"], project());
    assert!(
        code == 1 && out.contains("unknown option --project"),
        "{out}"
    );
    assert!(calls.is_empty(), "{calls:?}");
    assert_eq!(main_of(&f), before);
}

#[test]
fn a_worktree_with_uncommitted_changes_is_refused_before_anything_moves() {
    let f = fixture("land-uncommitted", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let before = main_of(&f);
    std::fs::write(one.join("half-done.txt"), "x\n").unwrap();
    let (code, out, calls) = run_land(&f, &one, &[&a], Cues::default());
    assert!(
        code == 1 && out.contains("has uncommitted changes") && out.contains("half-done.txt"),
        "{out}"
    );
    assert!(calls.is_empty(), "{calls:?}");
    assert_eq!(main_of(&f), before);
    assert_eq!(branch_of(&one).as_deref(), Some("a"));
    std::fs::remove_file(one.join("half-done.txt")).unwrap();
    std::fs::write(one.join("a.txt"), "changed\n").unwrap();
    let (code, out, _) = run_land(&f, &one, &[&a], Cues::default());
    assert!(
        code == 1 && out.contains("has uncommitted changes"),
        "{out}"
    );
    assert_eq!(git_out(&one, &["show", "HEAD:a.txt"]), "a");
    assert_eq!(
        std::fs::read_to_string(one.join("a.txt")).unwrap(),
        "changed\n"
    );
}

#[test]
fn unknown_options_and_missing_numbers_are_refused() {
    let f = fixture("land-options", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, out, _) = run_land(&f, &one, &["--bogus", &a], Cues::default());
    assert!(
        code == 1 && out.contains("refusing: unknown option --bogus"),
        "{out}"
    );
    let (code, out, _) = run_land(&f, &one, &["--done"], Cues::default());
    assert!(
        code == 1 && out.contains("--done needs an item number"),
        "{out}"
    );
    let (code, out, _) = run_land(
        &f,
        &one,
        &["--design", "2", "--design", "2", &a],
        Cues::default(),
    );
    assert!(
        code == 1 && out.contains("the landing names --design 2 twice"),
        "{out}"
    );
    let (code, out, _) = run_land(&f, &one, &["--design", "2,3", &a], Cues::default());
    assert!(code == 1 && out.contains("'2,3' is not an id"), "{out}");
    let (code, out, _) = run_land(&f, &one, &[], Cues::default());
    assert!(code == 1 && out.contains("usage: builds land"), "{out}");
}

#[test]
fn project_is_a_check_against_the_worktrees_project() {
    let f = fixture("land-project-flag", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, out, calls) = run_land(
        &f,
        &one,
        &["--project", "other", "--done", "7", &a],
        project(),
    );
    assert!(
        code == 1
            && out.contains("--project other does not match")
            && out.contains("is in project bae")
            && out.contains("reads the project from the worktree"),
        "{out}"
    );
    assert_eq!(calls, [format!("lists project-of {}", one.display())]);
    let (code, out, _) = run_land(&f, &one, &["--project"], project());
    assert!(
        code == 1 && out.contains("--project needs a project name"),
        "{out}"
    );
    let (code, out, _) = run_land(
        &f,
        &one,
        &["--project", "bae", "--project", "bae", &a],
        project(),
    );
    assert!(code == 1 && out.contains("names --project twice"), "{out}");
    let (code, out, calls) = run_land(
        &f,
        &one,
        &["--project", "bae", "--done", "7", &a],
        project(),
    );
    assert!(code == 0, "{out}");
    assert!(
        calls.iter().any(|c| c.starts_with("item landed")),
        "{calls:?}"
    );
}

#[test]
fn landing_a_commit_already_on_main_says_so_and_records_its_copy() {
    let f = fixture("land-already", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    run_land(&f, &one, &[&a], Cues::default());
    let (code, out, calls) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(
        code == 0 && out.contains("already on origin/main:"),
        "{out}"
    );
    // The stand-in's release leaves the claim, so the second landing
    // releases it again.
    assert_eq!(
        calls[calls.len() - 3..],
        [
            format!(
                "item landed {} --done 7 {a} --project bae --by agent A",
                one.display()
            ),
            // The first landing left the worktree detached at a.
            format!(
                "review landed {} --project bae --landing detached:{} {a} --by agent A",
                one.display(),
                &a[..7]
            ),
            format!("release {}", one.display())
        ]
    );
}

#[test]
fn a_landing_runs_only_the_whole_lines_and_says_each_ones_time() {
    // not-run.sh does not exist: a plain check line would fail if land ran it.
    let f = fixture("land-whole-only", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    run_land(&f, &one, &[&a], Cues::default());
    let (code, out, _) = run_land(&f, &two, &[&b], Cues::default());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("whole fake: ok, "), "{out}");
    assert!(!out.contains("skipped-by-land"), "{out}");
}

#[test]
fn every_failing_whole_line_is_named_with_its_output() {
    let f = fixture("land-whole-many", CHECK, None);
    std::fs::write(
        f.main.join(".checks"),
        "whole first * sh fake-check.sh\nwhole second * sh fake-check.sh\n",
    )
    .unwrap();
    git_out(&f.main, &["commit", "-q", "-a", "-m", "two checks"]);
    git_out(&f.main, &["push", "-q", "origin", "HEAD:main"]);
    let (one, a, two, b) = two_branches(&f);
    run_land(&f, &one, &[&a], Cues::default());
    let (code, out, _) = run_land(&f, &two, &[&b], Cues::default());
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
    let (one, a, _two, _b) = two_branches(&f);
    std::fs::write(f.main.join(".land-checks"), "true\n").unwrap();
    git_out(&f.main, &["add", ".land-checks"]);
    git_out(&f.main, &["commit", "-q", "-m", "old file"]);
    let old = git_out(&f.main, &["rev-parse", "HEAD"]);
    git_out(&f.main, &["push", "-q", "origin", "HEAD:main"]);
    let (code, out, _) = run_land(&f, &one, &[&a], Cues::default());
    assert!(
        code == 1 && out.contains(".land-checks") && out.contains("`whole` line"),
        "{out}"
    );
    assert_eq!(main_of(&f), old);
    assert_eq!(branch_of(&one).as_deref(), Some("a"));
}

#[test]
fn commits_the_review_does_not_take_are_refused_before_anything_is_fetched() {
    let f = fixture("land-review-refuses", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let dir = one.display();
    let (code, out, calls) = run_land(
        &f,
        &one,
        &["--done", "7", &a],
        Cues {
            refuse: "review check-landing",
            says: "no review run covers the commits this lands",
            ..project()
        },
    );
    assert!(
        code == 1
            && out.contains("LANDING REFUSED, land fetched and pushed nothing:")
            && out.contains("no review run covers"),
        "{out}"
    );
    assert_ne!(main_of(&f), a);
    assert_eq!(branch_of(&one).as_deref(), Some("a"));
    assert_eq!(
        calls.last().unwrap(),
        &format!("review check-landing {dir} --project bae --landing branch:a {a}")
    );
}

/// When the branch has no passing app build that covers what it lands, land
/// pushes nothing, checks the branch out again and prints why; it asks after
/// it built the result, with origin/main as it found it, the result and the
/// branch's last commit it lands.
#[test]
fn a_result_the_branch_has_no_app_build_for_is_refused_and_nothing_is_pushed() {
    let f = fixture("land-app-builds", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let before = main_of(&f);
    let dir = one.display();
    let (code, out, calls) = run_land(
        &f,
        &one,
        &["--done", "7", &a],
        Cues {
            refuse: "app-build --landing-build",
            says: "no passing mac app build built the branch",
            ..project()
        },
    );
    assert!(
        code == 1
            && out.contains("APP BUILDS DO NOT COVER the result, nothing pushed:")
            && out.contains("no passing mac app build built the branch"),
        "{out}"
    );
    assert_eq!(main_of(&f), before);
    assert_eq!(branch_of(&one).as_deref(), Some("a"));
    assert_eq!(head_of(&one), a);
    assert_eq!(
        calls.last().unwrap(),
        &format!("app-build --landing-build {dir} {before} {a} {a} a")
    );
}

/// The build the branch's builds name for a result on a newer main.
const REBUILD: &str = "echo \"builds app-build bae $5 macos --test mac CardTests\"";

/// When main moved under the branch's app build, land builds the result
/// itself, in the branch's worktree, with the command `app-build
/// --landing-build` prints, and pushes the result once the build passes.
#[test]
fn a_result_on_a_newer_main_is_built_in_the_worktree_before_it_is_pushed() {
    let f = fixture("land-app-rebuild", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    let (code, _, _) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(code == 0 && main_of(&f) == a);
    let (code, out, calls) = run_land(
        &f,
        &two,
        &["--done", "8", &b],
        Cues {
            landing_build: REBUILD,
            ..project()
        },
    );
    let result = main_of(&f);
    assert!(code == 0 && result != b, "{out}");
    assert_eq!(git_out(&f.origin, &["show", "main:a.txt"]), "a");
    assert_eq!(git_out(&f.origin, &["show", "main:b.txt"]), "b");
    let build = format!("app-build bae {result} macos --test mac CardTests");
    let asked = calls
        .iter()
        .position(|c| {
            c == &format!(
                "app-build --landing-build {} {a} {result} {b} b",
                two.display()
            )
        })
        .expect("land asks which build the result needs");
    let built = calls
        .iter()
        .position(|c| c == &build)
        .expect("land runs the build");
    let recorded = calls
        .iter()
        .position(|c| c.starts_with("item landed"))
        .unwrap();
    assert!(asked < built && built < recorded, "{calls:?}");
    assert_eq!(
        calls[built + 1],
        format!("app-build ran in {}", two.display())
    );
    assert!(out.contains(&format!("builds {build}")), "{out}");
}

/// A shots prefix an agent dropped is printed by land, with who dropped it and
/// why, ahead of the build it still runs.
#[test]
fn a_dropped_shots_prefix_is_printed_with_its_reason() {
    let f = fixture("land-app-dropped", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    let (code, _, _) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(code == 0 && main_of(&f) == a);
    let (code, out, _) = run_land(
        &f,
        &two,
        &["--done", "8", &b],
        Cues {
            landing_build: "echo 'shots dropped: the mac shots prefix mac.seek. needs no run before the landing; agent recorded why: the board moved'; echo \"builds app-build bae $5 macos\"",
            ..project()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("shots dropped: the mac shots prefix mac.seek. needs no run before the landing; agent recorded why: the board moved"),
        "{out}"
    );
    assert!(
        out.contains("before it pushes:\nbuilds app-build bae "),
        "{out}"
    );
}

/// A build of the result that fails refuses the landing with the build's
/// output: nothing is pushed and the branch is checked out again.
#[test]
fn a_result_whose_app_build_fails_is_refused_with_the_builds_output() {
    let f = fixture("land-app-rebuild-fails", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    let (code, _, _) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(code == 0 && main_of(&f) == a);
    let (code, out, calls) = run_land(
        &f,
        &two,
        &["--done", "8", &b],
        Cues {
            landing_build: REBUILD,
            refuse: "app-build bae",
            says: "CardTests failed: the card has no title",
            ..project()
        },
    );
    assert!(
        code == 1
            && out.contains("CardTests failed: the card has no title")
            && out.contains("APP BUILD FAILED on the result, nothing pushed"),
        "{out}"
    );
    assert_eq!(main_of(&f), a);
    assert_eq!(branch_of(&two).as_deref(), Some("b"));
    assert_eq!(head_of(&two), b);
    assert_eq!(git_out(&two, &["status", "--porcelain"]), "");
    assert!(
        !calls.iter().any(|c| c.starts_with("item landed")),
        "{calls:?}"
    );
}

/// A repo in no project records no app builds, so land asks nothing of them.
#[test]
fn a_repo_in_no_project_lands_without_asking_about_app_builds() {
    let f = fixture("land-app-builds-none", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, _, calls) = run_land(&f, &one, &[&a], Cues::default());
    assert!(code == 0 && main_of(&f) == a);
    assert!(
        !calls.iter().any(|call| call.starts_with("app-build")),
        "{calls:?}"
    );
}

#[test]
fn a_landing_the_review_takes_is_pushed_and_recorded_as_landed_after_the_items() {
    let f = fixture("land-review-takes", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let dir = one.display();
    let (code, _, calls) = run_land(&f, &one, &["--done", "7", "--part", "3", &a], project());
    assert!(code == 0 && main_of(&f) == a);
    let check = calls
        .iter()
        .position(|c| {
            c == &format!("review check-landing {dir} --project bae --landing branch:a {a}")
        })
        .expect("the review is asked before the landing");
    let items = calls
        .iter()
        .position(|c| c.starts_with("item check-landing"))
        .unwrap();
    assert!(items < check);
    assert_eq!(
        calls.last().unwrap(),
        &format!("review landed {dir} --project bae --landing branch:a {a} --by agent A")
    );
}

#[test]
fn a_failed_review_record_after_the_push_exits_with_the_rerun() {
    let f = fixture("land-review-record", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, out, _) = run_land(
        &f,
        &one,
        &["--done", "7", &a],
        Cues {
            refuse: "review landed",
            ..project()
        },
    );
    assert!(
        code == 1 && main_of(&f) == a && out.contains("LANDED, NOT RECORDED:"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "rerun: builds review landed {} --project bae --landing branch:a {a} --by 'agent A'",
            one.display()
        )),
        "{out}"
    );
}

#[test]
fn a_failing_check_stops_the_landing_before_its_record_and_a_repo_in_no_project_has_no_review() {
    let f = fixture("land-review-order", CHECK, None);
    let (one, a, two, b) = two_branches(&f);
    run_land(&f, &one, &["--done", "7", &a], project());
    let (code, _, calls) = run_land(&f, &two, &["--done", "8", &b], project());
    assert_eq!(code, 1);
    assert!(!calls.iter().any(|c| c.starts_with("review landed")));
    let g = fixture("land-review-none", "true\n", None);
    let (one, a, _two, _b) = two_branches(&g);
    let (code, _, calls) = run_land(&g, &one, &[&a], Cues::default());
    assert_eq!(code, 0);
    assert!(!calls.iter().any(|c| c.starts_with("review")));
}

#[test]
fn a_landing_that_main_moved_under_tries_again_from_a_fresh_fetch() {
    // The check pushes another clone's commit to origin the first time it
    // runs, as a landing in another worktree would, so origin refuses the
    // first push.
    let f = fixture(
        "land-retry",
        "[ -e {root}/moved ] || { touch {root}/moved; git -C {root}/other push -q origin HEAD:main; }\n",
        None,
    );
    let other = f.scratch.dir.join("other");
    git_out(
        &f.scratch.dir,
        &[
            "clone",
            "-q",
            f.origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    identify(&other);
    commit_file(&other, "c.txt", "c\n", "add c");
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let (code, out, calls) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(code == 0 && out.contains("push failed"), "{out}");
    let new = main_of(&f);
    assert_ne!(new, a, "the retry cherry-picked onto the moved main");
    assert_eq!(git_out(&f.origin, &["show", "main:a.txt"]), "a");
    assert_eq!(git_out(&f.origin, &["show", "main:c.txt"]), "c");
    assert!(calls.contains(&format!(
        "item landed {} --done 7 {new} --project bae --by agent A",
        one.display()
    )));
    assert_eq!(head_of(&one), new);
}

#[test]
fn landings_in_two_worktrees_run_at_the_same_time() {
    // Each landing's first check writes a mark into its own pipe and reads
    // the other landing's mark, so neither check passes until both landings
    // are in their checks. The script opens each pipe for reading and
    // writing, so no open waits, and the read gives up after a minute. A
    // second run of the check, after a refused push, passes at once.
    let f = fixture(
        "land-two-worktrees",
        r#"me=$(basename "$PWD")
[ -e {root}/checked-$me ] && exit 0
touch {root}/checked-$me
case $me in side-1) other=side-2 ;; *) other=side-1 ;; esac
exec 3<>{root}/pipe-$me 4<>{root}/pipe-$other
echo here >&3
read -t 60 mark <&4 || { echo "the landing in $other did not reach its check"; exit 1; }
"#,
        None,
    );
    for side in ["side-1", "side-2"] {
        let pipe = f.scratch.dir.join(format!("pipe-{side}"));
        let made = Command::new("mkfifo").arg(&pipe).status().unwrap();
        assert!(made.success(), "mkfifo {}", pipe.display());
    }
    let (one, a, two, b) = two_branches(&f);
    let (stub, _) = stub(&f, &Cues::default());
    let landing = |dir: &Path, sha: String| {
        let args = vec![dir.display().to_string(), sha];
        let stub = stub.clone();
        let places = f.scratch.places();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let result = land_recorded(&places, &stub, &args, &mut out, None);
            (result.is_ok(), String::from_utf8_lossy(&out).into_owned())
        })
    };
    let first = landing(&one, a);
    let second = landing(&two, b);
    let (ok_a, out_a) = first.join().unwrap();
    let (ok_b, out_b) = second.join().unwrap();
    assert!(ok_a && ok_b, "{out_a}\n{out_b}");
    let waits = |out: &str| out.contains("waiting for the landing in");
    assert!(!waits(&out_a) && !waits(&out_b), "{out_a}\n{out_b}");
    assert_eq!(git_out(&f.origin, &["show", "main:a.txt"]), "a");
    assert_eq!(git_out(&f.origin, &["show", "main:b.txt"]), "b");
}

#[test]
fn two_landings_started_together_in_one_worktree_run_one_after_the_other() {
    let f = fixture("land-lock", "sleep 1\n", None);
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let b = commit_file(&one, "b.txt", "b\n", "add b");
    let (stub, _) = stub(&f, &Cues::default());
    let landing = |sha: String| {
        let args = vec![one.display().to_string(), sha];
        let stub = stub.clone();
        let places = f.scratch.places();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let result = land_recorded(&places, &stub, &args, &mut out, None);
            (result.is_ok(), String::from_utf8_lossy(&out).into_owned())
        })
    };
    let first = landing(a.clone());
    let second = landing(b.clone());
    let (ok_a, out_a) = first.join().unwrap();
    let (ok_b, out_b) = second.join().unwrap();
    assert!(ok_a && ok_b, "{out_a}\n{out_b}");
    let waits = |out: &str| out.contains("waiting for the landing in");
    assert!(waits(&out_a) != waits(&out_b), "{out_a}\n{out_b}");
    assert_eq!(git_out(&f.origin, &["show", "main:a.txt"]), "a");
    assert_eq!(git_out(&f.origin, &["show", "main:b.txt"]), "b");
}

#[test]
fn a_waiting_landing_names_the_landing_it_waits_behind() {
    let f = fixture("land-lock-names", "true\n", None);
    let one = f.side(1, "a");
    let mut first = Vec::new();
    let held = lock_worktree(&one, &["--done".into(), "7".into()], &mut first).unwrap();
    let dir = one.clone();
    let waiter = std::thread::spawn(move || {
        let mut out = Vec::new();
        lock_worktree(&dir, &[], &mut out).unwrap();
        String::from_utf8_lossy(&out).into_owned()
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    drop(held);
    let said = waiter.join().unwrap();
    assert!(said.contains("landing --done 7"), "{said}");
    assert!(first.is_empty());
}

#[test]
fn a_landing_fast_forwards_the_main_checkout_and_keeps_the_persons_edits() {
    let f = fixture("land-main-forward", "true\n", None);
    let (one, a, two, b) = two_branches(&f);
    // The person's uncommitted edit, which neither landing touches.
    std::fs::write(f.main.join("base.txt"), "the person's\n").unwrap();
    let (code, out, _) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(
        code == 0 && out.contains("fast-forwarded the main checkout"),
        "{out}"
    );
    assert_eq!(head_of(&f.main), a);
    let (code, out, _) = run_land(&f, &two, &["--done", "8", &b], project());
    assert!(
        code == 0 && out.contains("fast-forwarded the main checkout"),
        "{out}"
    );
    assert_eq!(
        head_of(&f.main),
        main_of(&f),
        "the main checkout holds the cherry-picked commit too"
    );
    assert_eq!(branch_of(&f.main).as_deref(), Some("main"));
    assert_eq!(
        std::fs::read_to_string(f.main.join("base.txt")).unwrap(),
        "the person's\n"
    );
}

#[test]
fn a_main_checkout_that_cannot_fast_forward_leaves_the_landing_landed_and_says_how() {
    let f = fixture("land-main-blocked", "true\n", None);
    let command = format!("git -C {} merge --ff-only origin/main", f.main.display());
    // The person's own commit in the main checkout.
    let local = commit_file(&f.main, "local.txt", "local\n", "the person's commit");
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let (code, out, calls) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(
        code == 0
            && out.contains(&format!(
                "MAIN CHECKOUT BEHIND: the landing is on origin/main, but git did not fast-forward the main checkout {}",
                f.main.display()
            ))
            && out.contains(&command),
        "{out}"
    );
    assert_eq!(main_of(&f), a, "the commits stay on origin/main");
    assert!(calls.iter().any(|c| c.starts_with("item landed")));
    assert_eq!(
        head_of(&f.main),
        local,
        "land leaves the person's commit in place"
    );

    // The person's uncommitted file that the landing would overwrite.
    git_out(&f.main, &["reset", "-q", "--hard", "origin/main"]);
    std::fs::write(f.main.join("b.txt"), "the person's\n").unwrap();
    let two = f.side(2, "b");
    let b = commit_file(&two, "b.txt", "b\n", "add b");
    let (code, out, _) = run_land(&f, &two, &["--done", "8", &b], project());
    assert!(
        code == 0
            && out.contains("MAIN CHECKOUT BEHIND")
            && out.contains("git did not fast-forward")
            && out.contains(&command),
        "{out}"
    );
    assert_eq!(head_of(&f.main), a);
    assert_eq!(
        std::fs::read_to_string(f.main.join("b.txt")).unwrap(),
        "the person's\n"
    );

    // A main checkout on another branch.
    std::fs::remove_file(f.main.join("b.txt")).unwrap();
    git_out(&f.main, &["checkout", "-q", "-b", "other"]);
    let three = f.side(3, "c");
    let c = commit_file(&three, "c.txt", "c\n", "add c");
    let (code, out, _) = run_land(&f, &three, &["--done", "9", &c], project());
    assert!(
        code == 0
            && out.contains("is on branch other, not main")
            && out.contains("once the person checks out main there")
            && out.contains(&command),
        "{out}"
    );
    assert_eq!(head_of(&f.main), a);
}

#[test]
fn a_landing_in_a_project_records_its_steps_items_and_commits() {
    let f = fixture("land-profile", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, run) = profiled(&f, &one, &["--done", "7", &a], project());
    assert_eq!(code, 0);
    let run = run.expect("a repo in a project records its landing");
    assert_eq!(run.project, "bae");
    assert_eq!(run.repo, "repo");
    assert_eq!(run.branch.as_deref(), Some("a"));
    assert_eq!(run.by, "agent A");
    assert!(run.landed);
    assert_eq!(run.items, vec![Id(7)]);
    assert_eq!(run.commits.len(), 1);
    assert_eq!(run.commits[0].0, a);
    let kinds: Vec<StepKind> = run.steps.iter().map(|step| step.kind).collect();
    assert_eq!(
        kinds,
        vec![
            StepKind::Lock,
            StepKind::ListsCheck,
            StepKind::Fetch,
            StepKind::Pick,
            StepKind::Checks,
            StepKind::Push,
            StepKind::Finish,
            StepKind::Records,
            StepKind::Finish,
        ]
    );
}

/// A landing that changes the apps records its wait for its turn before its
/// fetch, and the app build of its result between its checks and its push.
#[test]
fn an_app_landing_records_its_wait_for_its_turn_and_the_app_build_of_its_result() {
    let f = app_fixture("land-profile-app");
    let (one, a) = app_branch(&f, 1, "a", "a.swift");
    let (code, run) = profiled(
        &f,
        &one,
        &["--done", "7", &a],
        Cues {
            landing_build: "echo \"builds app-build repo $5 macos\"",
            ..project()
        },
    );
    assert_eq!(code, 0);
    let kinds: Vec<StepKind> = run.unwrap().steps.iter().map(|step| step.kind).collect();
    assert_eq!(
        kinds,
        vec![
            StepKind::Lock,
            StepKind::ListsCheck,
            StepKind::AppWait,
            StepKind::Fetch,
            StepKind::Pick,
            StepKind::Checks,
            StepKind::AppBuild,
            StepKind::Push,
            StepKind::Finish,
            StepKind::Records,
            StepKind::Finish,
        ]
    );
}

#[test]
fn a_refused_landing_records_that_it_did_not_land() {
    let f = fixture("land-profile-refused", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, run) = profiled(
        &f,
        &one,
        &["--done", "7", &a],
        Cues {
            refuse: "item check-landing",
            ..project()
        },
    );
    assert_eq!(code, 1);
    let run = run.expect("a refusal after the project is read is recorded");
    assert!(!run.landed);
    let kinds: Vec<StepKind> = run.steps.iter().map(|step| step.kind).collect();
    assert_eq!(kinds, vec![StepKind::Lock, StepKind::ListsCheck]);
}

#[test]
fn a_repo_in_no_project_records_no_landing() {
    let f = fixture("land-profile-none", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, run) = profiled(&f, &one, &[&a], Cues::default());
    assert_eq!(code, 0);
    assert_eq!(run, None);
}

/// The store of project "bae" in <f>'s places, with one item, and the item.
fn store_with_item(f: &Fixture) -> (crate::lists::store::Store, Id) {
    use crate::lists::fixtures::{NOON, item, make_store_named, stamp};
    let store = make_store_named(&f.scratch.places(), "bae", &["repo"]);
    let id = store
        .write(&stamp(NOON), "add item", |db| Ok(item(db, vec![])))
        .unwrap();
    (store, id)
}

/// The landings recorded in the store at <database>, read past its schema
/// stamp, and the stamp.
fn landings_and_stamp(database: &Path) -> (i64, i64) {
    let conn = rusqlite::Connection::open(database).unwrap();
    let landed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM land_run WHERE landed = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let stamp: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    (landed, stamp)
}

/// A stamp no builds schema has, which stands for the schema of a newer
/// builds that an install moves the store to.
const NEWER_STAMP: i64 = 12_345;

#[test]
fn a_landing_records_its_run_before_the_install_its_main_checkout_runs_moves_the_store() {
    let f = fixture("land-record-before-install", "true\n", None);
    let (store, id) = store_with_item(&f);
    // The main checkout's post-merge hook stands for install.sh: it moves the
    // store to a newer schema, as `builds install` of a landed builds does.
    // Like the real hook, it acts only in the main checkout.
    let hooks = f.scratch.dir.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-merge");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/bash\n[ \"$(git rev-parse --show-toplevel)\" = \"$(cd '{main}' && pwd -P)\" ] || exit 0\nexec /usr/bin/sqlite3 '{db}' 'PRAGMA user_version = {NEWER_STAMP}'\n",
            main = f.main.display(),
            db = store.database().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    git_out(
        &f.main,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let (code, out, _) = run_land(&f, &one, &["--done", &id.0.to_string(), &a], project());
    assert!(
        code == 0 && out.contains("fast-forwarded the main checkout"),
        "{out}"
    );
    assert_eq!(
        landings_and_stamp(&store.database()),
        (1, NEWER_STAMP),
        "the landing wrote its run before the hook moved the store"
    );
}

#[test]
fn an_install_that_moves_the_store_waits_until_the_landing_records_its_run() {
    use crate::install_gate::{PerConnection, begin};
    let f = fixture("land-record-install-waits", "true\n", None);
    let (store, id) = store_with_item(&f);
    let places = f.scratch.places();
    let landing = PerConnection::new(places.clone(), 4242, "land", "10:00:00").unwrap();
    let stores = landing.hold().unwrap();
    // An install starts while the landing holds its use: it waits for the
    // use, then counts the landings the store holds and moves the store.
    let (events, said) = std::sync::mpsc::channel();
    let database = store.database();
    let install = std::thread::spawn({
        let places = places.clone();
        move || {
            let say = &mut |line: String| {
                let _ = events.send(line);
            };
            let closed = begin(&places, say).unwrap().close(say).unwrap();
            let (landed, _) = landings_and_stamp(&database);
            let conn = rusqlite::Connection::open(&database).unwrap();
            conn.pragma_update(None, "user_version", NEWER_STAMP)
                .unwrap();
            drop(closed);
            landed
        }
    });
    let waiting = said
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the install says it waits");
    assert!(waiting.contains("process 4242"), "{waiting}");
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let (stub, _) = stub(&f, &project());
    let args = vec![
        one.display().to_string(),
        "--done".to_string(),
        id.0.to_string(),
        a,
    ];
    let mut out = Vec::new();
    let result = land_recorded(&places, &stub, &args, &mut out, Some(stores));
    let out = String::from_utf8_lossy(&out);
    assert!(result.is_ok(), "{out}");
    assert_eq!(
        install.join().unwrap(),
        1,
        "the install moved the store only after the landing recorded its run"
    );
    assert_eq!(landings_and_stamp(&store.database()), (1, NEWER_STAMP));
}

const BOARDS: &str = "place design/mocks/button";

/// Side-1 on branch `a` with three commits, one for each round of a design
/// change: it adds a.txt, rewrites it, and adds b.txt.
fn three_rounds(f: &Fixture) -> (PathBuf, [String; 3]) {
    let dir = f.side(1, "a");
    let one = commit_file(&dir, "a.txt", "a\n", "draw the button");
    let two = commit_file(&dir, "a.txt", "a2\n", "redraw the button");
    let three = commit_file(&dir, "b.txt", "b\n", "draw the row");
    (dir, [one, two, three])
}

#[test]
fn a_design_change_with_several_amends_lands_as_one_commit_titled_by_the_change() {
    let f = fixture("land-design-squash", "true\n", None);
    let (dir, rounds) = three_rounds(&f);
    let base = main_of(&f);
    let tree = git_out(&dir, &["rev-parse", &format!("{}^{{tree}}", rounds[2])]);
    let shown = dir.display();
    let (code, out, calls) = run_land(
        &f,
        &dir,
        &["--design", "3", &rounds[0], &rounds[1], &rounds[2]],
        Cues {
            title: "Draw the button and the row",
            places: BOARDS,
            ..project()
        },
    );
    assert_eq!(code, 0, "{out}");
    let landed = main_of(&f);
    assert_eq!(
        git_out(
            &f.origin,
            &["rev-list", "--count", &format!("{base}..{landed}")]
        ),
        "1"
    );
    assert_eq!(
        git_out(&f.origin, &["log", "-1", "--format=%B", &landed]),
        "Draw the button and the row"
    );
    assert_eq!(
        git_out(&f.origin, &["rev-parse", &format!("{landed}^{{tree}}")]),
        tree
    );
    assert_eq!(
        git_out(&f.origin, &["rev-parse", &format!("{landed}^")]),
        base
    );
    assert!(
        calls.contains(&"design title 3 --project bae".to_string()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("design check 3 {shown} {landed} --project bae")),
        "{calls:?}"
    );
    assert!(calls.contains(&format!(
        "design landed 3 {shown} {landed} --project bae --by agent A"
    )));
    assert!(
        out.contains("squashed 3 commits of design change 3"),
        "{out}"
    );
}

#[test]
fn a_design_change_with_one_commit_lands_as_it_is() {
    let f = fixture("land-design-one-commit", "true\n", None);
    let dir = f.side(1, "a");
    let only = commit_file(&dir, "a.txt", "a\n", "draw the button");
    let (code, out, calls) = run_land(
        &f,
        &dir,
        &["--design", "3", &only],
        Cues {
            title: "Draw the button",
            places: BOARDS,
            ..project()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(main_of(&f), only);
    assert!(
        !calls.iter().any(|c| c.starts_with("design title")),
        "{calls:?}"
    );
}

#[test]
fn a_squash_that_would_change_the_tree_is_refused_before_anything_moves() {
    let f = fixture("land-design-squash-tree", "true\n", None);
    let (dir, rounds) = three_rounds(&f);
    let base = main_of(&f);
    // The middle round is not named, so the squash would drop its rewrite.
    let (code, out, calls) = run_land(
        &f,
        &dir,
        &["--design", "3", &rounds[0], &rounds[2]],
        Cues {
            title: "Draw the button and the row",
            places: BOARDS,
            ..project()
        },
    );
    assert!(
        code == 1 && out.contains("cannot squash") && out.contains("not named"),
        "{out}"
    );
    assert_eq!(main_of(&f), base);
    assert_eq!(head_of(&dir), rounds[2], "the branch stays where it was");
    assert_eq!(branch_of(&dir).as_deref(), Some("a"));
    assert!(
        !calls.iter().any(|c| c.starts_with("design check")),
        "{calls:?}"
    );
}

#[test]
fn a_squash_is_refused_when_the_branch_has_commits_after_the_last_one_named() {
    let f = fixture("land-design-squash-later", "true\n", None);
    let (dir, rounds) = three_rounds(&f);
    let base = main_of(&f);
    let (code, out, _) = run_land(
        &f,
        &dir,
        &["--design", "3", &rounds[0], &rounds[1]],
        Cues {
            title: "Draw the button",
            places: BOARDS,
            ..project()
        },
    );
    assert!(code == 1 && out.contains("has commits after"), "{out}");
    assert_eq!(main_of(&f), base);
    assert_eq!(head_of(&dir), rounds[2]);
}

#[test]
fn a_design_landing_with_an_item_squashes_its_design_commits_too() {
    let f = fixture("land-design-squash-item", "true\n", None);
    let (dir, rounds) = three_rounds(&f);
    let base = main_of(&f);
    let shown = dir.display();
    let (code, out, calls) = run_land(
        &f,
        &dir,
        &[
            "--done", "921", "--design", "3", &rounds[0], &rounds[1], &rounds[2],
        ],
        Cues {
            title: "Draw the button and the row",
            places: BOARDS,
            ..project()
        },
    );
    assert_eq!(code, 0, "{out}");
    let landed = main_of(&f);
    assert_eq!(
        git_out(
            &f.origin,
            &["rev-list", "--count", &format!("{base}..{landed}")]
        ),
        "1"
    );
    assert_eq!(
        git_out(&f.origin, &["log", "-1", "--format=%B", &landed]),
        "Draw the button and the row"
    );
    assert!(
        calls.contains(&format!(
            "item landed {shown} --done 921 {landed} --project bae --by agent A"
        )),
        "{calls:?}"
    );
    assert!(calls.contains(&format!(
        "design landed 3 {shown} {landed} --project bae --by agent A"
    )));
}

#[test]
fn commits_that_change_app_files_land_one_by_one_with_their_item() {
    let f = fixture("land-design-squash-app", "true\n", None);
    let (dir, rounds) = three_rounds(&f);
    let base = main_of(&f);
    let (code, out, calls) = run_land(
        &f,
        &dir,
        &[
            "--done", "921", "--design", "3", &rounds[0], &rounds[1], &rounds[2],
        ],
        Cues {
            title: "Draw the button and the row",
            places: BOARDS,
            refuse: "design only",
            says: "the commits also change files that are no design file: src/x.rs",
            ..project()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert_eq!(main_of(&f), rounds[2]);
    assert_eq!(
        git_out(
            &f.origin,
            &["rev-list", "--count", &format!("{base}..{}", rounds[2])]
        ),
        "3"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("design title")),
        "{calls:?}"
    );
}

/// The fixture, with an `.app-build` on main whose mac lane builds what
/// changes under app/.
fn app_fixture(name: &str) -> Fixture {
    let f = fixture(name, "true\n", None);
    commit_file(
        &f.main,
        ".app-build",
        "step mac macos ./macos.sh\non app/ macos\n",
        "build the apps",
    );
    git_out(&f.main, &["push", "-q", "origin", "HEAD:main"]);
    f
}

/// Side worktree <n> on a new branch <branch> with the file app/<file>,
/// which the mac lane builds, committed.
fn app_branch(f: &Fixture, n: usize, branch: &str, file: &str) -> (PathBuf, String) {
    let dir = f.side(n, branch);
    std::fs::create_dir_all(dir.join("app")).unwrap();
    let sha = commit_file(
        &dir,
        &format!("app/{file}"),
        "app\n",
        &format!("change {file}"),
    );
    (dir, sha)
}

/// Holds the turn of the landings that change the apps of <f>'s repo, as a
/// landing ahead of the next one would.
fn hold_app_turn(f: &Fixture) -> std::fs::File {
    let common = git_out(
        &f.main,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    take_turn(
        &Path::new(&common).join(APP_TURN_FILE),
        "the test's landing",
        "process 1 landing ahead",
        &mut Vec::new(),
    )
    .unwrap()
}

/// Lands <args> in <dir> on a thread of its own, with the stand-in `builds`
/// at <stub> and <out> for what the landing says; returns whether it landed.
fn land_on_thread(
    f: &Fixture,
    stub: &Path,
    dir: &Path,
    args: &[&str],
    mut out: impl Write + Send + 'static,
) -> std::thread::JoinHandle<bool> {
    let mut all = vec![dir.display().to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    let places = f.scratch.places();
    let stub = stub.to_path_buf();
    std::thread::spawn(move || land_recorded(&places, &stub, &all, &mut out, None).is_ok())
}

/// A landing that changes the apps waits, before it fetches, while another
/// landing that changes them holds the repo's turn, names that landing, and
/// lands once the turn is free.
#[test]
fn a_landing_that_changes_the_apps_waits_for_the_one_ahead_and_names_it() {
    let f = app_fixture("land-app-turn");
    let before = main_of(&f);
    let (one, a) = app_branch(&f, 1, "a", "a.swift");
    let held = hold_app_turn(&f);
    let (stub, _) = stub(&f, &project());
    let (reader, writer) = std::io::pipe().unwrap();
    let landing = land_on_thread(&f, &stub, &one, &["--done", "7", &a], writer);
    let mut lines = std::io::BufReader::new(reader).lines();
    let waiting = line_starting(&mut lines, "waiting for");
    assert!(
        waiting.contains("the landing that changes the apps of repo ahead of this one")
            && waiting.contains("process 1 landing ahead"),
        "{waiting}"
    );
    assert_eq!(main_of(&f), before, "the landing waits before it pushes");
    drop(held);
    let rest: Vec<String> = lines.map(|line| line.unwrap()).collect();
    assert!(landing.join().unwrap(), "{rest:?}");
    assert_eq!(main_of(&f), a);
}

/// A landing that changes no app takes no turn, so it lands while another
/// landing holds the turn of those that change the apps.
#[test]
fn a_landing_that_changes_no_app_lands_while_an_app_landing_holds_its_turn() {
    let f = app_fixture("land-app-turn-other");
    let one = f.side(1, "a");
    let a = commit_file(&one, "notes.txt", "notes\n", "add notes");
    let _held = hold_app_turn(&f);
    let (stub, _) = stub(&f, &project());
    let landing = land_on_thread(&f, &stub, &one, &["--done", "7", &a], Vec::new());
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || done.send(landing.join().unwrap()));
    let landed = ended
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the landing does not wait for the app landing's turn");
    assert!(landed && main_of(&f) == a);
}

/// Reads <lines> until one starts with <start>, and returns that line.
fn line_starting(lines: &mut impl Iterator<Item = std::io::Result<String>>, start: &str) -> String {
    lines
        .map(|line| line.unwrap())
        .find(|line| line.starts_with(start))
        .unwrap_or_else(|| panic!("no line starts with {start:?}"))
}

/// Two landings that change the apps run one after the other, and each
/// builds its result once: the second waits while the first builds, then
/// builds on the first's pushed result, so no build is of a result that never
/// reached main.
#[test]
fn two_app_landings_each_build_once_and_the_second_waits_for_the_first() {
    let f = app_fixture("land-app-turns");
    let (one, a) = app_branch(&f, 1, "a", "a.swift");
    let (two, b) = app_branch(&f, 2, "b", "b.swift");
    let go = f.scratch.dir.join("go");
    let made = Command::new("mkfifo").arg(&go).status().unwrap();
    assert!(made.success(), "mkfifo {}", go.display());
    let (stub, _) = stub(
        &f,
        &Cues {
            landing_build: "grep -qx \"$5\" '{root}/built' 2>/dev/null || echo \"builds app-build repo $5 macos\"",
            // The first build waits until the test lets it finish.
            app_build: "[ -e '{root}/built' ] || read go < '{root}/go'; echo \"$3\" >> '{root}/built'",
            ..project()
        },
    );
    let (first_out, first_in) = std::io::pipe().unwrap();
    let first = land_on_thread(&f, &stub, &one, &["--done", "7", &a], first_in);
    let mut first_lines = std::io::BufReader::new(first_out).lines();
    line_starting(&mut first_lines, "another landing that changed the apps");
    let (second_out, second_in) = std::io::pipe().unwrap();
    let second = land_on_thread(&f, &stub, &two, &["--done", "8", &b], second_in);
    let mut second_lines = std::io::BufReader::new(second_out).lines();
    let waiting = line_starting(&mut second_lines, "waiting for");
    assert!(waiting.contains(&a), "{waiting}");
    std::fs::write(&go, "go\n").unwrap();
    let first_rest: Vec<String> = first_lines.map(|line| line.unwrap()).collect();
    let second_rest: Vec<String> = second_lines.map(|line| line.unwrap()).collect();
    assert!(first.join().unwrap(), "{first_rest:?}");
    assert!(second.join().unwrap(), "{second_rest:?}");
    assert_eq!(git_out(&f.origin, &["show", "main:app/a.swift"]), "app");
    assert_eq!(git_out(&f.origin, &["show", "main:app/b.swift"]), "app");
    let built = std::fs::read_to_string(f.scratch.dir.join("built")).unwrap();
    let built: Vec<&str> = built.lines().collect();
    let on_main = git_out(&f.origin, &["rev-list", "main"]);
    assert_eq!(built.len(), 2, "{built:?}");
    assert!(
        built
            .iter()
            .all(|commit| on_main.lines().any(|on| on == *commit)),
        "{built:?} are not all on main:\n{on_main}"
    );
}
