//! What a command that makes work checks before it takes the store's lock:
//! that the briefs it names exist, and that a request gives `work` or
//! `no_work`.

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::Result;
use crate::host::clone_path;
use crate::lists::item::ItemDraft;
use crate::places::Places;

/// The file of <brief>, `path` or `path#section`. An absolute path is itself.
/// A relative path is looked for under the main checkout of each of <repos>,
/// the repos of the command's project (`<home>/dev/<repo>`), because briefs
/// are plans' files that no commit carries (`plans/` is ignored) and the
/// worktree a command runs in may belong to another repo than the one that
/// holds the brief. The file must exist; a relative path that exactly one
/// main checkout holds is that file, and a path that none or several hold is
/// refused, naming the places looked in or the files found. This reads the
/// file system, so a command runs it before it takes the lock.
pub fn find_brief(places: &Places, repos: &[String], brief: &str) -> Result<PathBuf> {
    let path = brief.split('#').next().unwrap_or("");
    if path.is_empty() {
        fail!("brief '{brief}' names no file");
    }
    if Path::new(path).is_absolute() {
        return if Path::new(path).is_file() {
            Ok(PathBuf::from(path))
        } else {
            fail!("brief {brief} does not exist: no file {path}")
        };
    }
    let candidates: Vec<PathBuf> = repos
        .iter()
        .map(|repo| places.home.join(clone_path(repo)).join(path))
        .collect();
    let mut found: Vec<PathBuf> = candidates.iter().filter(|c| c.is_file()).cloned().collect();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => fail!(
            "brief {brief} does not exist: no file at {}; give the brief as an absolute path, or put it in the main checkout of one of the project's repos",
            list(&candidates)
        ),
        _ => fail!(
            "brief {brief} is ambiguous: the main checkouts of {} hold it ({}); give the brief as an absolute path",
            repos.join(" and "),
            list(&found)
        ),
    }
}

fn list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The briefs of <drafts> exist (see `find_brief`).
pub fn require_draft_briefs(places: &Places, repos: &[String], drafts: &[ItemDraft]) -> Result<()> {
    drafts
        .iter()
        .filter_map(|draft| draft.brief.as_deref())
        .try_for_each(|brief| find_brief(places, repos, brief).map(|_| ()))
}

/// A request's `work` (item drafts) or `no_work` (why there is none): one
/// of them, never both and never neither.
pub fn work_or_why(
    work: Option<Vec<ItemDraft>>,
    no_work: Option<String>,
) -> Result<Vec<ItemDraft>> {
    match (work, no_work) {
        (Some(_), Some(_)) => fail!("give `work` or `no_work`, not both"),
        (None, None) => {
            fail!("give `work` (the items this makes) or `no_work` with the reason there is none")
        }
        (None, Some(why)) if why.trim().is_empty() => fail!("`no_work` needs its reason"),
        (None, Some(_)) => Ok(Vec::new()),
        (Some(work), None) => Ok(work),
    }
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
