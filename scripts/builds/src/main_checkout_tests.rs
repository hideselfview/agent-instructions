use super::*;
use crate::testing::{Scratch, git_out};

fn identify(dir: &Path) {
    git_out(dir, &["config", "user.name", "t"]);
    git_out(dir, &["config", "user.email", "t@t"]);
    git_out(dir, &["config", "commit.gpgsign", "false"]);
}

fn commit_file(dir: &Path, name: &str, message: &str) {
    std::fs::write(dir.join(name), format!("{name}\n")).unwrap();
    git_out(dir, &["add", name]);
    git_out(dir, &["commit", "-q", "-m", message]);
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// Health names a project's main checkout whose main lacks commits that
/// origin/main has, with the command that fast-forwards it, and says why when
/// git cannot fast-forward it there; a repo with no main checkout on this Mac
/// is no finding.
#[test]
fn health_lists_a_project_main_checkout_behind_origin_main() {
    let scratch = Scratch::new("main-checkout-behind");
    let tool = scratch.tool(0);
    crate::lists::lists_words(&tool, &words("init demo --repos demo,absent --by tester")).unwrap();
    let origin = scratch.dir.join("origin.git");
    git_out(
        &scratch.dir,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    let main = tool.places.home.join("dev/demo");
    std::fs::create_dir_all(main.parent().unwrap()).unwrap();
    git_out(
        &scratch.dir,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            main.to_str().unwrap(),
        ],
    );
    identify(&main);
    commit_file(&main, "base.txt", "base");
    git_out(&main, &["push", "-q", "origin", "HEAD:main"]);
    let found = || {
        findings(
            &tool.places.home,
            &crate::lists::store::load_all(&tool.places),
        )
    };
    assert!(found().is_empty(), "a current main checkout is no finding");

    // Another clone lands a commit, and a fetch here sees it.
    let other = scratch.dir.join("other");
    git_out(
        &scratch.dir,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    identify(&other);
    commit_file(&other, "landed.txt", "landed");
    git_out(&other, &["push", "-q", "origin", "HEAD:main"]);
    git_out(&main, &["fetch", "-q", "origin"]);
    let command = format!("git -C {} merge --ff-only origin/main", main.display());
    let behind = found();
    assert_eq!(behind.len(), 1, "{behind:?}");
    assert_eq!(behind[0].kind, crate::health::FindingKind::MainCheckout);
    assert_eq!(behind[0].advice.command.as_deref(), Some(command.as_str()));
    assert!(
        behind[0].advice.text.starts_with(&format!(
            "checkout: the main checkout {} lacks 1 commit that origin/main has",
            main.display()
        )),
        "{behind:?}"
    );

    // The person's own commit stops the fast-forward.
    commit_file(&main, "local.txt", "the person's commit");
    let blocked = found();
    assert_eq!(blocked.len(), 1, "{blocked:?}");
    assert!(
        blocked[0]
            .advice
            .text
            .contains("has 1 commit on main that origin/main lacks")
            && blocked[0]
                .advice
                .text
                .contains("pushes or removes those commits"),
        "{blocked:?}"
    );

    git_out(&main, &["reset", "-q", "--hard", "origin/main"]);
    assert!(
        found().is_empty(),
        "a fast-forwarded main checkout is no finding"
    );
}

/// Health lists a main checkout whose last install failed, with the step and
/// the command that installs again, until an install succeeds; no record is no
/// finding.
#[test]
fn health_lists_a_failed_install_until_an_install_succeeds() {
    let scratch = Scratch::new("main-checkout-install");
    let tool = scratch.tool(0);
    crate::lists::lists_words(&tool, &words("init demo --repos demo --by tester")).unwrap();
    let main = tool.places.home.join("dev/demo");
    std::fs::create_dir_all(&main).unwrap();
    git_out(&main, &["init", "-q", "-b", "main"]);
    identify(&main);
    commit_file(&main, "base.txt", "base");
    git_out(&main, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let found = || {
        findings(
            &tool.places.home,
            &crate::lists::store::load_all(&tool.places),
        )
    };
    assert!(found().is_empty(), "no install record is no finding");

    let record = main.join(".git/builds-install");
    let sha = git_out(&main, &["rev-parse", "HEAD"]);
    std::fs::write(
        &record,
        format!("failed {sha} building builds with cargo\n"),
    )
    .unwrap();
    let failed = found();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(
        failed[0].advice.command.as_deref(),
        Some(format!("{}/install.sh", main.display()).as_str())
    );
    assert!(
        failed[0].advice.text.contains(&sha[..8])
            && failed[0].advice.text.contains("building builds with cargo")
            && failed[0].advice.text.contains("older than origin/main"),
        "{failed:?}"
    );

    std::fs::write(&record, format!("ok {sha}\n")).unwrap();
    assert!(found().is_empty(), "a successful install clears it");

    // A record that names no step is broken, and health says so.
    std::fs::write(&record, format!("failed {sha}\n")).unwrap();
    let broken = found();
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert!(
        broken[0]
            .advice
            .text
            .contains("could not read how the last install")
            && broken[0].advice.text.contains("neither `ok COMMIT`"),
        "{broken:?}"
    );
}

/// A repo's main checkout is `<home>/dev/<repo>`, listed once however many
/// projects hold the repo, and a repo with no such folder has none.
#[test]
fn main_checkouts_names_each_repo_folder_that_exists_once() {
    let scratch = Scratch::new("main-checkouts");
    std::fs::create_dir_all(scratch.dir.join("dev/a")).unwrap();
    std::fs::create_dir_all(scratch.dir.join("dev/b")).unwrap();
    let found = main_checkouts(&scratch.dir, ["b", "a", "b", "absent"].into_iter());
    assert_eq!(
        found,
        [scratch.dir.join("dev/a"), scratch.dir.join("dev/b")]
    );
}
