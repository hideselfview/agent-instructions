//! The squash of a design landing (decision #292): the commits of one design
//! change, one for each round of redrawing on its review page, become one
//! commit whose message is the change's title.
//!
//! The squash changes no file. `squash` replays each named commit onto the
//! first one's tree and builds the new commit on the first one's parent only
//! when the replay comes out byte-identical to the last commit's tree, so the
//! review and the person's approval, which cover files, still hold.

use std::path::Path;
use std::process::Stdio;

use boards::git;

/// What git printed for <args> in <dir>, without trailing newlines, or git's
/// own words when it failed.
fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = git(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    let text = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .trim_end_matches('\n')
            .to_string()
    };
    if output.status.success() {
        Ok(text(&output.stdout))
    } else {
        Err(format!(
            "git {} failed: {}{}",
            args.join(" "),
            text(&output.stdout),
            text(&output.stderr)
        ))
    }
}

/// Builds the commit that holds <shas> (full ids, oldest first, two or more)
/// as one, with <title> as its message and the parent of the first of them as
/// its parent, and returns its id. Refuses commits that are out of order, a
/// first commit with no parent, and a replay of the commits whose tree is not
/// the last commit's tree, which happens when a commit between them is not
/// named. It moves no branch.
pub fn squash(dir: &Path, shas: &[String], title: &str) -> Result<String, String> {
    let (Some(first), Some(last)) = (shas.first(), shas.last()) else {
        return Err("land squashes no commits".to_string());
    };
    for pair in shas.windows(2) {
        let ordered = git(dir)
            .args(["merge-base", "--is-ancestor", &pair[0], &pair[1]])
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !ordered || pair[0] == pair[1] {
            return Err(format!(
                "{} is not an ancestor of {}; name a design change's commits oldest first",
                pair[0], pair[1]
            ));
        }
    }
    let parent = run(dir, &["rev-parse", "--verify", &format!("{first}^")])
        .map_err(|_| format!("{first} has no parent commit to squash onto"))?;
    let mut tree = run(dir, &["rev-parse", &format!("{first}^{{tree}}")])?;
    for sha in &shas[1..] {
        let base = format!("--merge-base={sha}^");
        tree = run(dir, &["merge-tree", "--write-tree", &base, &tree, sha])
            .map_err(|said| format!("replaying {sha} onto the commits before it failed: {said}"))?;
    }
    let want = run(dir, &["rev-parse", &format!("{last}^{{tree}}")])?;
    if tree != want {
        return Err(format!(
            "squashing the named commits gives tree {tree}, not the tree {want} of {last}: a commit between them is not named, and the squash would change files the review and the approval cover"
        ));
    }
    run(dir, &["commit-tree", &tree, "-p", &parent, "-m", title])
}
