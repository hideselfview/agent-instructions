//! Which design places a set of commits changes: the changed paths from git,
//! matched against the globs of the repo's `design_places`.
//!
//! A glob is `/`-separated; a segment may hold `*` (any characters but `/`)
//! and `?` (one character). A glob that ends in `/` names folders: every path
//! below a folder it matches belongs to that one place, counted once, and the
//! place is the folder with its trailing `/`. Any other glob names files.
//! A changed path that is no place but sits under a directory the globs start
//! in (`design/`) is a shared design file: it belongs to no board or
//! component.

use std::collections::BTreeSet;
use std::path::Path;

use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::Result;

/// What a set of commits changes among the design files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Design places: a file path, or a folder path with a trailing `/`.
    pub places: BTreeSet<String>,
    /// Changed files under the globs' root directories that are no place.
    pub shared: BTreeSet<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.places.is_empty() && self.shared.is_empty()
    }
}

struct Glob {
    segments: Vec<String>,
    folder: bool,
}

impl Glob {
    fn parse(text: &str) -> Glob {
        let folder = text.ends_with('/');
        let segments = text.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
        Glob { segments, folder }
    }

    /// The place <path> belongs to under this glob.
    fn place(&self, path: &str) -> Option<String> {
        let parts: Vec<&str> = path.split('/').collect();
        let n = self.segments.len();
        let fits = if self.folder { parts.len() > n } else { parts.len() == n };
        if !fits || !self.segments.iter().zip(&parts).all(|(glob, part)| segment_matches(glob, part)) {
            return None;
        }
        Some(if self.folder { format!("{}/", parts[..n].join("/")) } else { path.to_string() })
    }

    /// The first segment when it is plain text and more follows: the
    /// directory the design files live in.
    fn root(&self) -> Option<&str> {
        let first = self.segments.first()?;
        (self.segments.len() > 1 && !first.contains(['*', '?'])).then_some(first.as_str())
    }
}

fn segment_matches(glob: &str, text: &str) -> bool {
    fn go(glob: &[char], text: &[char]) -> bool {
        match glob.split_first() {
            None => text.is_empty(),
            Some(('*', rest)) => (0..=text.len()).any(|skip| go(rest, &text[skip..])),
            Some(('?', rest)) => !text.is_empty() && go(rest, &text[1..]),
            Some((c, rest)) => text.first() == Some(c) && go(rest, &text[1..]),
        }
    }
    go(&glob.chars().collect::<Vec<_>>(), &text.chars().collect::<Vec<_>>())
}

/// The places and shared files among <paths>, by <globs>.
pub fn classify(globs: &[String], paths: &[String]) -> Changes {
    let globs: Vec<Glob> = globs.iter().map(|g| Glob::parse(g)).collect();
    let roots: BTreeSet<&str> = globs.iter().filter_map(Glob::root).collect();
    let mut changes = Changes::default();
    for path in paths {
        if let Some(place) = globs.iter().find_map(|glob| glob.place(path)) {
            changes.places.insert(place);
        } else if path.split_once('/').is_some_and(|(first, _)| roots.contains(first)) {
            changes.shared.insert(path.clone());
        }
    }
    changes
}

/// The paths commit <sha> changes in the checkout at <top>. A merge counts
/// against each parent.
pub fn commit_paths(top: &Path, sha: &str) -> Result<Vec<String>> {
    let out = git_text(
        git(top).args(["diff-tree", "--no-commit-id", "--name-only", "-r", "--root", "-m", "-z", sha, "--"]),
        &format!("list the files {sha} changes"),
    )?;
    Ok(out.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect())
}

/// What <shas> change among the design files of the repo whose globs are
/// <globs>.
pub fn changes_of(top: &Path, globs: &[String], shas: &[String]) -> Result<Changes> {
    let mut paths = Vec::new();
    for sha in shas {
        paths.extend(commit_paths(top, sha)?);
    }
    Ok(classify(globs, &paths))
}

/// The commits of <range> (`origin/main..HEAD`), oldest first; a range with
/// none is refused.
pub fn commits_of_range(top: &Path, range: &str) -> Result<Vec<String>> {
    let out = git_text(git(top).args(["rev-list", "--reverse", range, "--"]), &format!("list the commits of {range}"))?;
    let shas: Vec<String> = out.lines().map(str::to_string).collect();
    if shas.is_empty() {
        fail!("the range {range} holds no commits");
    }
    Ok(shas)
}

/// The full hash of commit <sha>.
pub fn full_sha(top: &Path, sha: &str) -> Result<String> {
    let out = git_text(
        git(top).args(["rev-parse", "--verify", "--quiet", &format!("{sha}^{{commit}}")]),
        &format!("find commit {sha}"),
    )?;
    Ok(out.trim().to_string())
}

/// Is commit <sha> on origin/main, as the checkout at <top> knows it?
pub fn on_origin_main(top: &Path, sha: &str) -> Result<bool> {
    let status = git(top)
        .args(["merge-base", "--is-ancestor", sha, "origin/main"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| crate::fail::Fail::Message(format!("running git to test {sha} against origin/main: {error}")))?;
    match status.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => fail!(
            "git failed to test {sha} against origin/main: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ),
    }
}

#[cfg(test)]
#[path = "design_paths_tests.rs"]
mod tests;
