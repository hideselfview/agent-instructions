use super::*;
use crate::testing::{Scratch, git_out};

/// The landing in <f>, which records its run in the store of its project in
/// <f>'s places, when there is one.
fn land(f: &Fixture, program: &Path, args: &[String], out: &mut dyn Write) -> Result<()> {
    let places = f.scratch.places();
    land_recorded(&places, program, args, &mut Report::to(out), None)
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
/// is what `design title` prints and `coverage` the shell words that answer
/// `app-build --landing-coverage <worktree> <branch> <ref> <commit>...`,
/// `{root}` in them standing for the scratch folder.
#[derive(Default)]
struct Cues {
    project: &'static str,
    title: &'static str,
    places: &'static str,
    refuse: &'static str,
    says: &'static str,
    coverage: &'static str,
    /// A file name: `design only` for one commit that changes exactly this
    /// file says the commit changes no design files, as it does for a
    /// commit of spec lines alone.
    no_design_file: &'static str,
}

/// Writes the stand-in `builds` for <cues> and returns it with the file it
/// logs each command to.
fn stub(f: &Fixture, cues: &Cues) -> (PathBuf, PathBuf) {
    let or_true = |words: &'static str| if words.is_empty() { "true" } else { words };
    let root = f.scratch.dir.display().to_string();
    let stub = f.scratch.dir.join("stub-builds");
    let log = f.scratch.dir.join("calls.log");
    let script = format!(
        "#!/bin/bash\necho \"$*\" >> '{log}'\n[ '{refuse}' = \"$1 $2\" ] && {{ echo '{says}'; exit 1; }}\nif [ \"$1 $2\" = 'design only' ] && [ \"$5\" = '--project' ] && [ -n '{no_design_file}' ] && [ \"$(git -C \"$3\" show --name-only --format= \"$4\")\" = '{no_design_file}' ]; then echo 'the commits change no design files'; exit 1; fi\ncase \"$1 $2\" in\n  'lists project-of') echo '{project}' ;;\n  'review key') k=$(git -C \"$3\" symbolic-ref -q --short HEAD) && echo \"branch:$k\" || echo \"detached:$(git -C \"$3\" rev-parse --short HEAD)\" ;;\n  'design places') printf '%s' '{places}' ;;\n  'design title') echo '{title}' ;;\n  'app-build --landing-coverage') {coverage} ;;\nesac\nexit 0\n",
        log = log.display(),
        refuse = cues.refuse,
        says = if cues.says.is_empty() {
            format!("stub refuses {}", cues.refuse)
        } else {
            cues.says.replace('\'', "'\\''")
        },
        project = cues.project,
        title = cues.title,
        no_design_file = cues.no_design_file,
        places = cues.places,
        coverage = or_true(cues.coverage).replace("{root}", &root),
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
        Err(error) => panic!("landing failed: {}", error.text()),
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
    let Ended { result, run, .. } = land_profiled(&stub, &all, &mut Report::to(&mut out));
    let code = match result {
        Ok(()) => 0,
        Err(Fail::Exit(code)) => code,
        Err(error) => panic!("landing failed: {}", error.text()),
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
fn a_landing_that_names_no_commit_takes_the_branchs_commits_oldest_first_and_says_which() {
    let f = fixture("land-no-shas", CHECK, None);
    let one = f.side(1, "a");
    let first = commit_file(&one, "a.txt", "a\n", "add a");
    let second = commit_file(&one, "c.txt", "c\n", "add c");
    let (code, out, _) = run_land(&f, &one, &[], Cues::default());
    assert!(code == 0 && main_of(&f) == second, "{out}");
    let at = |sha: &str, subject: &str| out.find(&format!("{} {subject}", &sha[..7]));
    let (a, c) = (at(&first, "add a"), at(&second, "add c"));
    assert!(
        a.is_some() && c.is_some() && a < c,
        "land says which commits it took, oldest first: {out}"
    );
    let (code, out, _) = run_land(&f, &one, &[], Cues::default());
    assert!(
        code == 1 && out.contains("no commit that origin/main lacks"),
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
            format!("review key {dir} --project bae"),
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
fn spec_sections_need_no_design_change_but_land_with_one() {
    let f = fixture("land-spec-only", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, _, _) = run_land(
        &f,
        &one,
        &["--done", "7", &a],
        Cues {
            places: "spec spec/bae.md#Sync",
            ..project()
        },
    );
    assert_eq!(code, 0, "a spec section alone needs no design change");
}

#[test]
fn spec_sections_land_through_a_design_change_beside_the_items() {
    let f = fixture("land-spec-design", "true\n", None);
    let (one, a, _two, _b) = two_branches(&f);
    let (code, _, calls) = run_land(
        &f,
        &one,
        &["--done", "7", "--design", "3", &a],
        Cues {
            places: "spec spec/bae.md#Sync",
            ..project()
        },
    );
    assert_eq!(code, 0);
    assert!(calls.contains(&format!(
        "design check 3 {} {a} --project bae",
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
    let mut out = Vec::new();
    let refused = land(&f, Path::new("unused"), &[], &mut out);
    assert!(
        matches!(refused, Err(Fail::Exit(1)))
            && String::from_utf8_lossy(&out).contains("usage: builds land"),
        "{}",
        String::from_utf8_lossy(&out)
    );
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
/// refuses before it fetches anything: origin/main as the worktree knows it
/// stays where it was, nothing is pushed, and the branch stays checked out.
#[test]
fn a_branch_with_no_covering_app_build_is_refused_before_land_fetches() {
    let f = app_fixture("land-app-uncovered");
    let (one, a) = app_branch(&f, 1, "a", "a.swift");
    let known = git_out(&f.main, &["rev-parse", "origin/main"]);
    // Main moves on origin, which a fetch would show.
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
    commit_file(&other, "elsewhere.txt", "x\n", "move main");
    git_out(&other, &["push", "-q", "origin", "HEAD:main"]);
    let moved = main_of(&f);
    assert_ne!(moved, known);
    let dir = one.display();
    let (code, out, calls) = run_land(
        &f,
        &one,
        &["--done", "7", &a],
        Cues {
            refuse: "app-build --landing-coverage",
            says: "no passing mac app build built the branch",
            ..project()
        },
    );
    assert!(
        code == 1
            && out.contains("APP BUILDS DO NOT COVER the branch, nothing fetched or pushed:")
            && out.contains("no passing mac app build built the branch"),
        "{out}"
    );
    assert_eq!(main_of(&f), moved);
    assert_eq!(git_out(&f.main, &["rev-parse", "origin/main"]), known);
    assert_eq!(branch_of(&one).as_deref(), Some("a"));
    assert_eq!(head_of(&one), a);
    assert_eq!(
        calls.last().unwrap(),
        &format!("app-build --landing-coverage {dir} {a} a {a}")
    );
}

/// A branch with a passing build of its own commit lands after another app
/// landing moved main: land asks only for the branch's coverage and runs no
/// app build of the result.
#[test]
fn a_branch_with_a_passing_build_lands_after_another_app_landing_moved_main() {
    let f = app_fixture("land-app-covered");
    let (one, a) = app_branch(&f, 1, "a", "a.swift");
    let (two, b) = app_branch(&f, 2, "b", "b.swift");
    let (code, _, _) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(code == 0 && main_of(&f) == a);
    let (code, out, calls) = run_land(&f, &two, &["--done", "8", &b], project());
    assert_eq!(code, 0, "{out}");
    assert_eq!(git_out(&f.origin, &["show", "main:app/a.swift"]), "app");
    assert_eq!(git_out(&f.origin, &["show", "main:app/b.swift"]), "app");
    let builds: Vec<&String> = calls
        .iter()
        .filter(|call| call.starts_with("app-build"))
        .collect();
    assert_eq!(
        builds,
        [&format!(
            "app-build --landing-coverage {} {b} b {b}",
            two.display()
        )]
    );
    let asked = calls
        .iter()
        .position(|c| c.starts_with("app-build --landing-coverage"))
        .unwrap();
    let recorded = calls
        .iter()
        .position(|c| c.starts_with("item landed"))
        .unwrap();
    assert!(asked < recorded, "{calls:?}");
}

/// A landing that names several commits asks for the coverage of the branch
/// at the last one and gives all of them as its own.
#[test]
fn the_coverage_check_gets_the_branch_the_ref_and_every_commit_landed() {
    let f = app_fixture("land-app-coverage-args");
    let one = f.side(1, "a");
    std::fs::create_dir_all(one.join("app")).unwrap();
    let first = commit_file(&one, "app/a.swift", "app\n", "add a");
    let last = commit_file(&one, "app/b.swift", "app\n", "add b");
    let (code, out, calls) = run_land(&f, &one, &["--done", "7", &first, &last], project());
    assert_eq!(code, 0, "{out}");
    assert!(
        calls.contains(&format!(
            "app-build --landing-coverage {} {last} a {first} {last}",
            one.display()
        )),
        "{calls:?}"
    );
}

/// What the coverage check prints about dropped shots prefixes and pairs the
/// base never rendered, land prints too.
#[test]
fn what_the_coverage_check_prints_about_shots_is_printed() {
    let f = app_fixture("land-app-dropped");
    let (one, a) = app_branch(&f, 1, "a", "a.swift");
    let said = "shots dropped: the mac shots prefix mac.seek. needs no run before the landing; agent recorded why: the board moved";
    let (code, out, _) = run_land(
        &f,
        &one,
        &["--done", "7", &a],
        Cues {
            coverage: "echo 'shots dropped: the mac shots prefix mac.seek. needs no run before the landing; agent recorded why: the board moved'",
            ..project()
        },
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(said), "{out}");
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
            let result = land_recorded(&places, &stub, &args, &mut Report::to(&mut out), None);
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
            let result = land_recorded(&places, &stub, &args, &mut Report::to(&mut out), None);
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

/// While a landing has other commits checked out in its worktree, the turn it
/// holds names the branch it started on, which health reads.
#[test]
fn a_running_landing_names_its_branch_while_it_checks_out_other_commits() {
    // The check runs on the landing's result.
    let f = fixture(
        "land-turn-names-branch",
        r#"cp "$(git rev-parse --absolute-git-dir)/land.lock" {root}/turn
git branch --show-current > {root}/checked-out
"#,
        None,
    );
    let (one, a, two, b) = two_branches(&f);
    // Main moves, so the second landing cherry-picks onto it.
    assert_eq!(run_land(&f, &one, &[&a], Cues::default()).0, 0);
    let (code, said, _) = run_land(&f, &two, &[&b], Cues::default());
    assert_eq!(code, 0, "{said}");
    let read = |name: &str| std::fs::read_to_string(f.scratch.dir.join(name)).unwrap();
    assert_eq!(read("checked-out").trim(), "");
    assert_eq!(record_branch(&read("turn")), Some("b"), "{}", read("turn"));
}

#[test]
fn a_waiting_landing_names_the_landing_it_waits_behind() {
    let f = fixture("land-lock-names", "true\n", None);
    let one = f.side(1, "a");
    let mut first = Vec::new();
    let held = lock_worktree(
        &one,
        &["--done".into(), "7".into()],
        &mut Report::to(&mut first),
    )
    .unwrap();
    let dir = one.clone();
    let waiter = std::thread::spawn(move || {
        let mut out = Vec::new();
        lock_worktree(&dir, &[], &mut Report::to(&mut out)).unwrap();
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
fn a_landing_whose_install_failed_says_so_and_names_the_step() {
    let f = fixture("land-install-failed", "true\n", None);
    // The main checkout's post-merge hook runs install.sh, which records how
    // it ended in the git folder (install.sh); this one fails at its build.
    let hook = f.main.join(".git/hooks/post-merge");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho \"failed $(git rev-parse HEAD) building builds with cargo\" > \"$(git rev-parse --git-common-dir)/builds-install\"\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let one = f.side(1, "a");
    let a = commit_file(&one, "a.txt", "a\n", "add a");
    let (code, out, _) = run_land(&f, &one, &["--done", "7", &a], project());
    assert!(
        code == 0
            && out.contains("INSTALL FAILED:")
            && out.contains("building builds with cargo")
            && out.contains(&a[..8])
            && out.contains(&format!("{}/install.sh", f.main.display())),
        "{out}"
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
    let result = land_recorded(
        &places,
        &stub,
        &args,
        &mut Report::to(&mut out),
        Some(stores),
    );
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
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("review check-landing") && c.ends_with("--squashed")),
        "{calls:?}"
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
    assert!(
        !calls.iter().any(|c| c.ends_with("--squashed")),
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

/// Lands the design rounds of `three_rounds` with a commit of spec.md, which
/// is no design file of the change, added `before` them or after them.
fn land_rounds_with_a_spec_commit(name: &str, before: bool) {
    let f = fixture(name, "true\n", None);
    let dir = f.side(1, "a");
    let base = main_of(&f);
    let mut shas = Vec::new();
    if before {
        shas.push(commit_file(&dir, "spec.md", "spec\n", "Say how rows wrap"));
    }
    shas.push(commit_file(&dir, "a.txt", "a\n", "draw the button"));
    shas.push(commit_file(&dir, "a.txt", "a2\n", "redraw the button"));
    if !before {
        shas.push(commit_file(&dir, "spec.md", "spec\n", "Say how rows wrap"));
    }
    let tree = git_out(&dir, &["rev-parse", "HEAD^{tree}"]);
    let mut args = vec!["--design", "3"];
    args.extend(shas.iter().map(String::as_str));
    let (code, out, _) = run_land(
        &f,
        &dir,
        &args,
        Cues {
            title: "Draw the button",
            places: BOARDS,
            no_design_file: "spec.md",
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
        "2",
        "the two rounds become one commit and the spec commit stays: {out}"
    );
    let messages = git_out(
        &f.origin,
        &["log", "--format=%s", &format!("{base}..{landed}")],
    );
    let want = if before {
        "Draw the button\nSay how rows wrap"
    } else {
        "Say how rows wrap\nDraw the button"
    };
    assert_eq!(messages, want);
    assert_eq!(
        git_out(&f.origin, &["rev-parse", &format!("{landed}^{{tree}}")]),
        tree
    );
}

#[test]
fn a_spec_commit_after_the_design_rounds_keeps_its_own_commit() {
    land_rounds_with_a_spec_commit("land-squash-spec-after", false);
}

#[test]
fn a_spec_commit_before_the_design_rounds_keeps_its_own_commit() {
    land_rounds_with_a_spec_commit("land-squash-spec-before", true);
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
/// changes under app/, whose android lane builds what changes under
/// android/, and whose both lanes build what changes under shared/.
fn app_fixture(name: &str) -> Fixture {
    app_fixture_checking(name, "true\n")
}

/// `app_fixture` with <checks> as the fake whole-tree check's script.
fn app_fixture_checking(name: &str, checks: &str) -> Fixture {
    let f = fixture(name, checks, None);
    commit_file(
        &f.main,
        ".app-build",
        "step mac macos ./macos.sh\non app/ macos\nstep android gradle ./gradle.sh\non android/ gradle\non shared/ gradle macos\n",
        "build the apps",
    );
    git_out(&f.main, &["push", "-q", "origin", "HEAD:main"]);
    f
}

/// Side worktree <n> on a new branch <branch> with the file <path>
/// committed.
fn branch_with(f: &Fixture, n: usize, branch: &str, path: &str) -> (PathBuf, String) {
    let dir = f.side(n, branch);
    std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
    let sha = commit_file(&dir, path, "app\n", &format!("change {path}"));
    (dir, sha)
}

/// Side worktree <n> on a new branch <branch> with the file app/<file>,
/// which the mac lane builds, committed.
fn app_branch(f: &Fixture, n: usize, branch: &str, file: &str) -> (PathBuf, String) {
    branch_with(f, n, branch, &format!("app/{file}"))
}

/// Two landings that change the apps, in different worktrees, run at the
/// same time: each one's first check waits for the other's, so neither check
/// passes until both landings hold no turn and sit in their checks. Both push
/// from the same origin/main, origin refuses the second push, and that landing
/// fetches again and lands on the first's result.
#[test]
fn two_app_landings_in_different_worktrees_run_at_once_and_the_refused_push_fetches_again() {
    let f = app_fixture_checking(
        "land-app-two-worktrees",
        r#"me=$(basename "$PWD")
[ -e {root}/checked-$me ] && exit 0
touch {root}/checked-$me
case $me in side-1) other=side-2 ;; *) other=side-1 ;; esac
exec 3<>{root}/pipe-$me 4<>{root}/pipe-$other
echo here >&3
read -t 60 mark <&4 || { echo "the landing in $other did not reach its check"; exit 1; }
"#,
    );
    for side in ["side-1", "side-2"] {
        let pipe = f.scratch.dir.join(format!("pipe-{side}"));
        let made = Command::new("mkfifo").arg(&pipe).status().unwrap();
        assert!(made.success(), "mkfifo {}", pipe.display());
    }
    let (one, a) = app_branch(&f, 1, "a", "a.swift");
    let (two, b) = app_branch(&f, 2, "b", "b.swift");
    let (stub, log) = stub(&f, &project());
    let landing = |dir: &Path, item: &str, sha: String| {
        let args = vec![dir.display().to_string(), "--done".into(), item.into(), sha];
        let (stub, places) = (stub.clone(), f.scratch.places());
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let result = land_recorded(&places, &stub, &args, &mut Report::to(&mut out), None);
            (result.is_ok(), String::from_utf8_lossy(&out).into_owned())
        })
    };
    let first = landing(&one, "7", a);
    let second = landing(&two, "8", b);
    let (ok_a, out_a) = first.join().unwrap();
    let (ok_b, out_b) = second.join().unwrap();
    assert!(ok_a && ok_b, "{out_a}\n{out_b}");
    let refused = [&out_a, &out_b]
        .into_iter()
        .filter(|out| out.contains("push failed"))
        .count();
    assert_eq!(refused, 1, "{out_a}\n{out_b}");
    assert!(!out_a.contains("waiting for") && !out_b.contains("waiting for"));
    assert_eq!(git_out(&f.origin, &["show", "main:app/a.swift"]), "app");
    assert_eq!(git_out(&f.origin, &["show", "main:app/b.swift"]), "app");
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|call| call.starts_with("app-build"))
            .count(),
        2,
        "{calls}"
    );
    assert!(
        calls
            .lines()
            .filter(|call| call.starts_with("app-build"))
            .all(|call| call.starts_with("app-build --landing-coverage")),
        "{calls}"
    );
}

/// A landing that changes the apps takes no turn and builds nothing, so its
/// run holds the steps of any landing.
#[test]
fn an_app_landing_records_the_steps_of_any_landing() {
    let f = app_fixture("land-profile-app");
    let (one, a) = app_branch(&f, 1, "a", "a.swift");
    let (code, run) = profiled(&f, &one, &["--done", "7", &a], project());
    assert_eq!(code, 0);
    let kinds: Vec<StepKind> = run.unwrap().steps.iter().map(|step| step.kind).collect();
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
