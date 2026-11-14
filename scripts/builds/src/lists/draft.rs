//! What a command that makes work checks before it takes the store's lock:
//! that the briefs it names exist, and that a request gives `work` or
//! `no_work`.

use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::lists::item::ItemDraft;

/// Where a relative brief path is looked for. Briefs are plans' files, which
/// no commit carries (`plans/` is ignored), so it is the main checkout of the
/// repo the command runs in; an absolute path is itself. A command with a
/// brief and no checkout is refused.
pub fn brief_base(cwd: &Path) -> Result<std::path::PathBuf> {
    match crate::checkout::Checkout::at(cwd)? {
        Some(checkout) => Ok(checkout.main),
        None => fail!(
            "{} is in no git checkout, so a relative brief path has nothing to be relative to; give the brief as an absolute path",
            cwd.display()
        ),
    }
}

/// A brief path, `path` or `path#section`: the file must exist. A relative
/// path is looked for under <base>. This reads the file system, so a command
/// runs it before it takes the lock.
pub fn require_brief(base: &Path, brief: &str) -> Result<()> {
    let path = brief.split('#').next().unwrap_or("");
    if path.is_empty() {
        fail!("brief '{brief}' names no file");
    }
    let file = base.join(path);
    if !file.is_file() {
        fail!("brief {brief} does not exist: no file {}", file.display());
    }
    Ok(())
}

/// <brief> exists, as a command running in <cwd> sees it: an absolute path is
/// itself, a relative one is under the main checkout of <cwd>'s repo. Only a
/// relative brief needs a checkout.
pub fn require_brief_from(cwd: &Path, brief: &str) -> Result<()> {
    if Path::new(brief).is_absolute() {
        return require_brief(Path::new("/"), brief);
    }
    require_brief(&brief_base(cwd)?, brief)
}

/// The briefs of <drafts> exist (see `require_brief_from`).
pub fn require_draft_briefs(cwd: &Path, drafts: &[ItemDraft]) -> Result<()> {
    drafts
        .iter()
        .filter_map(|draft| draft.brief.as_deref())
        .try_for_each(|brief| require_brief_from(cwd, brief))
}

impl ItemDraft {
    /// The brief's file exists (see `require_brief`).
    pub fn require_brief(&self, base: &Path) -> Result<()> {
        match &self.brief {
            Some(brief) => require_brief(base, brief),
            None => Ok(()),
        }
    }
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
