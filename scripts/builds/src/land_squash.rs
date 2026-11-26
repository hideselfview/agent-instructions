//! The squash of a design landing (decision #292): the commits of one design
//! change, one for each round of redrawing on its review page, become one
//! commit whose message is the change's title.
//!
//! A landing's other commits, such as a spec edit for another item, keep their
//! own commits: `squash_run` squashes only the run of design commits and
//! rebuilds the commits after it on the squashed one, each with its own tree,
//! author and message.
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

/// Builds the commits that land <shas> (full ids, oldest first) with the
/// design commits <design> of them squashed into one commit titled <title>
/// (`squash`), and returns the new ids in the order of <shas>, each commit
/// outside <design> as it was. A commit after the design commits is built again on the
/// squashed commit with its own tree, author and message, so every commit's
/// tree stays what it was. It moves no branch.
pub fn squash_run(
    dir: &Path,
    shas: &[String],
    design: std::ops::Range<usize>,
    title: &str,
) -> Result<Vec<String>> {
    let mut landed: Vec<String> = shas[..design.start].to_vec();
    let mut parent = squash(dir, &shas[design.clone()], title)?;
    landed.push(parent.clone());
    for sha in &shas[design.end..] {
        let tree = run(dir, &["rev-parse", &format!("{sha}^{{tree}}")])?;
        let message = run(dir, &["log", "-1", "--format=%B", sha])?;
        let who = run(dir, &["log", "-1", "--format=%an%n%ae%n%aI", sha])?;
        let [name, email, date] = who.lines().collect::<Vec<_>>()[..] else {
            fail!("git did not print the author of {sha}");
        };
        let mut command = boards::git(dir);
        command
            .args(["commit-tree", &tree, "-p", &parent, "-m", &message])
            .env("GIT_AUTHOR_NAME", name)
            .env("GIT_AUTHOR_EMAIL", email)
            .env("GIT_AUTHOR_DATE", date);
        parent = boards::git_text(&mut command, &format!("rebuild {sha} in {}", dir.display()))?
            .trim_end_matches('\n')
            .to_string();
        landed.push(parent.clone());
    }
    Ok(landed)
}
