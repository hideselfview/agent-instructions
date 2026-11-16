use super::*;
use crate::testing::Scratch;

fn git_out(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
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
    crate::lists::cmd_lists(&tool, &words("init demo --repos demo,absent --by tester")).unwrap();
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
