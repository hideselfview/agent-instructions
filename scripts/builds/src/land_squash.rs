//! The squash of a design landing (decision #292): the commits of one design
//! change, one for each round of redrawing on its review page, become one
//! commit whose message is the change's title.
//!
//! The squash changes no file. `squash` replays each named commit onto the
//! first one's tree and builds the new commit on the first one's parent only
//! when the replay comes out byte-identical to the last commit's tree, so the
//! review and the person's approval, which cover files, still hold.

use std::path::Path;

use crate::fail;
use crate::fail::Result;
use crate::git::text;

/// What git prints for <args> in <dir>, without the trailing newline.
fn run(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(text(dir, args)?.trim_end_matches('\n').to_string())
}

/// Builds the commit that holds <shas> (full ids, oldest first, two or more)
/// as one, with <title> as its message and the parent of the first of them as
/// its parent, and returns its id. Refuses commits that are out of order, a
/// first commit with no parent, and a replay of the commits whose tree is not
/// the last commit's tree, which happens when a commit between them is not
/// named. It moves no branch.
pub fn squash(dir: &Path, shas: &[String], title: &str) -> Result<String> {
    let (Some(first), Some(last)) = (shas.first(), shas.last()) else {
        fail!("land squashes no commits");
    };
    for pair in shas.windows(2) {
        // `a` is an ancestor of `b` exactly when their merge base is `a`.
        if run(dir, &["merge-base", &pair[0], &pair[1]])? != pair[0] {
            fail!(
                "{} is not an ancestor of {}; name a design change's commits oldest first",
                pair[0],
                pair[1]
            );
        }
        if pair[0] == pair[1] {
            fail!("{} is named twice", pair[0]);
        }
    }
    let parent = run(dir, &["rev-parse", "--verify", &format!("{first}^")])?;
    let mut tree = run(dir, &["rev-parse", &format!("{first}^{{tree}}")])?;
    for sha in &shas[1..] {
        let base = format!("--merge-base={sha}^");
        tree = run(dir, &["merge-tree", "--write-tree", &base, &tree, sha])?;
    }
    let want = run(dir, &["rev-parse", &format!("{last}^{{tree}}")])?;
    if tree != want {
        fail!(
            "squashing the named commits gives tree {tree}, not the tree {want} of {last}: a commit between them is not named, and the squash would change files the review and the approval cover"
        );
    }
    run(dir, &["commit-tree", &tree, "-p", &parent, "-m", title])
}
