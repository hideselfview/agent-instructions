//! The git commands a review runs on a branch's commits: they summarize the
//! commits of a range and list the paths those commits change.

use std::path::Path;

use crate::fail::Result;

/// The full SHA of each of <revs>, in order; a rev that names no commit fails.
fn full_shas(repo: &Path, revs: &[String]) -> Result<Vec<String>> {
    revs.iter()
        .map(|rev| {
            Ok(crate::git::text(
                repo,
                &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
            )?
            .trim()
            .to_string())
        })
        .collect()
}

/// The commits past <base> up to <head>, oldest first, as a reviewer reads
/// them: each one's short sha and message, then the files it changes with the
/// lines it adds and removes. The test cases are left out
/// (`without_test_cases`). A change that moves between commits changes no
/// file of the branch's net change, and shows only here.
pub fn summary(repo: &Path, base: &str, head: &str) -> Result<String> {
    let without = without_test_cases();
    crate::git::text(
        repo,
        &[
            "log",
            "--reverse",
            "--no-renames",
            "--numstat",
            "--format=commit %h%n%B",
            &format!("{base}..{head}"),
            "--",
            ".",
            &without,
        ],
    )
}

/// The pathspec that leaves the review's test cases (`cases::DIR`) out of
/// every diff and list of paths a review reads: a case holds old code on
/// purpose, so no group reviews it.
pub fn without_test_cases() -> String {
    format!(":(exclude){}", crate::review::cases::DIR)
}

/// Every path a commit of <revs> adds, changes or deletes (both paths of a
/// rename), except the test cases, each once, in the order first touched.
pub fn paths(repo: &Path, revs: &[String]) -> Result<Vec<String>> {
    if revs.is_empty() {
        return Ok(Vec::new());
    }
    let shas = full_shas(repo, revs)?;
    let mut args: Vec<&str> = vec![
        "log",
        "--no-walk=unsorted",
        "--no-renames",
        "--name-only",
        "--format=",
    ];
    args.extend(shas.iter().map(String::as_str));
    let without = without_test_cases();
    args.extend(["--", ".", &without]);
    let mut all: Vec<String> = Vec::new();
    for line in crate::git::text(repo, &args)?.lines() {
        if !line.is_empty() && !all.iter().any(|p| p == line) {
            all.push(line.to_string());
        }
    }
    Ok(all)
}

/// The paths each of <shas> adds, changes or deletes, in the order given.
pub fn paths_of_each(repo: &Path, shas: &[String]) -> Result<Vec<Vec<String>>> {
    shas.iter()
        .map(|sha| paths(repo, std::slice::from_ref(sha)))
        .collect()
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
