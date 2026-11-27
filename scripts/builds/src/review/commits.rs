//! The git commands a review runs on a branch's commits: they summarize the
//! commits of a range and list the paths those commits change, leaving out
//! the folders no review reads (`Unread`).

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

/// The folders of one repo that no review reads: the review's test cases
/// (`cases::DIR`), since a case holds old code on purpose, and the folders the
/// project's file names for the repo
/// (`lists::project_file::unread_folders`). builds leaves them out of every
/// diff and list of changed paths that a review reads, and `builds land` asks
/// the review about the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Unread {
    folders: Vec<String>,
}

impl Unread {
    /// The unread folders of a repo whose project file names <folders>, each
    /// without its last slash.
    pub fn new(folders: &[String]) -> Unread {
        Unread {
            folders: folders.to_vec(),
        }
    }

    fn folders(&self) -> impl Iterator<Item = &str> {
        std::iter::once(crate::review::cases::DIR).chain(self.folders.iter().map(String::as_str))
    }

    /// Whether <path> is in a folder no review reads.
    pub fn covers(&self, path: &str) -> bool {
        self.folders().any(|folder| {
            path.strip_prefix(folder)
                .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// The pathspecs that leave the unread folders out of what a git command
    /// reads. They go after `--`, beside the paths the command reads.
    pub fn excludes(&self) -> Vec<String> {
        self.folders()
            .map(|folder| format!(":(top,exclude,literal){folder}"))
            .collect()
    }

    /// The `--` and pathspecs that make a git command read every path but
    /// those of the unread folders.
    pub fn rest(&self) -> Vec<String> {
        let mut args = vec!["--".to_string(), ":(top)".to_string()];
        args.extend(self.excludes());
        args
    }
}

/// The commits past <base> up to <head>, oldest first, as a reviewer reads
/// them: each one's short sha and message, then the files it changes with the
/// lines it adds and removes. The <unread> folders are left out. A change that
/// moves between commits changes no file of the branch's net change, and shows
/// only here.
pub fn summary(repo: &Path, base: &str, head: &str, unread: &Unread) -> Result<String> {
    let range = format!("{base}..{head}");
    let rest = unread.rest();
    let mut args = vec![
        "log",
        "--reverse",
        "--no-renames",
        "--numstat",
        "--format=commit %h%n%B",
        &range,
    ];
    args.extend(rest.iter().map(String::as_str));
    crate::git::text(repo, &args)
}

/// Every path a commit of <revs> adds, changes or deletes (both paths of a
/// rename), except those in the <unread> folders, each once, in the order
/// first touched.
pub fn paths(repo: &Path, revs: &[String], unread: &Unread) -> Result<Vec<String>> {
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
    let rest = unread.rest();
    args.extend(rest.iter().map(String::as_str));
    let mut all: Vec<String> = Vec::new();
    for line in crate::git::text(repo, &args)?.lines() {
        if !line.is_empty() && !all.iter().any(|p| p == line) {
            all.push(line.to_string());
        }
    }
    Ok(all)
}

/// The paths each of <shas> adds, changes or deletes, in the order given.
pub fn paths_of_each(repo: &Path, shas: &[String], unread: &Unread) -> Result<Vec<Vec<String>>> {
    shas.iter()
        .map(|sha| paths(repo, std::slice::from_ref(sha), unread))
        .collect()
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
