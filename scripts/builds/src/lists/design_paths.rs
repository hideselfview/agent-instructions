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
//!
//! A board file the commits delete is no place that needs an answer: it is
//! listed as deleted. Three more kinds of path belong to a design change. A source glob names
//! the shared design-system sources (the component sources, the tokens): any
//! of them can change any component's drawing, so they are listed apart and
//! the design change names the components that change. A spec glob names the
//! spec files; when the commits are a design change (they change a place, a
//! source or a shared file), each section of a spec file (by heading) they
//! touch is a place, written `path#heading`. A path none of this covers is
//! outside the design files, and a spec file in commits that are no design
//! change is outside them too.
//!
//! A drawing (`drawing_at`) is the git object each changed board, component
//! folder and source is at one commit. Comparing two drawings tells whether
//! two commits hold those files byte for byte the same, whatever their hashes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::Lists;

/// A repo's globs: its design places, its shared sources and its spec files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Globs {
    pub places: Vec<String>,
    pub sources: Vec<String>,
    pub spec: Vec<String>,
}

impl Globs {
    /// The globs of <repo> in <lists>.
    pub fn of(lists: &Lists, repo: &str) -> Globs {
        let get = |map: &std::collections::BTreeMap<String, Vec<String>>| {
            map.get(repo).cloned().unwrap_or_default()
        };
        Globs {
            places: get(&lists.design_places),
            sources: get(&lists.design_sources),
            spec: get(&lists.spec_files),
        }
    }

    /// Does <key> name a design place of these globs: a board file, or a
    /// component folder with its trailing `/`?
    pub fn names_place(&self, key: &str) -> bool {
        self.places.iter().any(|glob| Glob::parse(glob).names(key))
    }

    /// Does <path> name one of these globs' spec files?
    pub fn names_spec_file(&self, path: &str) -> bool {
        self.spec.iter().any(|glob| Glob::parse(glob).matches(path))
    }
}

/// What a place is, read from how its key is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    /// A board file.
    Board,
    /// A component folder, `path/`.
    Component,
    /// A spec section, `path#heading`.
    Spec,
}

impl PlaceKind {
    pub fn of(key: &str) -> PlaceKind {
        if key.contains('#') {
            PlaceKind::Spec
        } else if key.ends_with('/') {
            PlaceKind::Component
        } else {
            PlaceKind::Board
        }
    }
}

/// What a set of commits changes among the design files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Design places: a file path, or a folder path with a trailing `/`.
    pub places: BTreeSet<String>,
    /// Changed shared design-system sources, by file.
    pub sources: BTreeSet<String>,
    /// Changed spec sections, `path#heading`; only when the commits are a
    /// design change.
    pub spec: BTreeSet<String>,
    /// Changed files under the globs' root directories that are none of
    /// the above.
    pub shared: BTreeSet<String>,
    /// Board files the commits delete: a board place that is no file at the
    /// last commit. It needs no answer, canvas link or place on the review
    /// page, and is not among `places`.
    pub deleted: BTreeSet<String>,
    /// Changed paths that are no design file.
    pub outside: BTreeSet<String>,
    /// Changed spec files, before their sections are read.
    spec_files: BTreeSet<String>,
}

impl Changes {
    /// Do the commits change design files at all? A spec file alone is no
    /// design change: the spec moves with every commit.
    pub fn is_design_change(&self) -> bool {
        !(self.places.is_empty()
            && self.deleted.is_empty()
            && self.sources.is_empty()
            && self.shared.is_empty())
    }

    /// The changed paths whose bytes a board's drawing shows: the places
    /// (board files and component folders) and the shared design-system
    /// sources. Shared files that are no source and spec sections change no
    /// drawing.
    pub fn drawn(&self) -> impl Iterator<Item = &String> {
        self.places.iter().chain(&self.sources)
    }

    /// Every place the design change must resolve: the places and the spec
    /// sections.
    pub fn resolvable(&self) -> BTreeSet<&String> {
        self.places.iter().chain(&self.spec).collect()
    }

    /// Is every changed path a design file (so the landing has no app code
    /// in it)?
    pub fn only_design(&self) -> bool {
        self.is_design_change() && self.outside.is_empty()
    }
}

struct Glob {
    segments: Vec<String>,
    folder: bool,
}

impl Glob {
    fn parse(text: &str) -> Glob {
        let folder = text.ends_with('/');
        let segments = text
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        Glob { segments, folder }
    }

    /// The place <path> belongs to under this glob.
    fn place(&self, path: &str) -> Option<String> {
        let parts: Vec<&str> = path.split('/').collect();
        let n = self.segments.len();
        let fits = if self.folder {
            parts.len() > n
        } else {
            parts.len() == n
        };
        if !fits
            || !self
                .segments
                .iter()
                .zip(&parts)
                .all(|(glob, part)| segment_matches(glob, part))
        {
            return None;
        }
        Some(if self.folder {
            format!("{}/", parts[..n].join("/"))
        } else {
            path.to_string()
        })
    }

    /// Is <path> in this glob's place?
    fn matches(&self, path: &str) -> bool {
        self.place(path).is_some()
    }

    /// Is <key> exactly a place of this glob: what `place` returns for a
    /// path it matches?
    fn names(&self, key: &str) -> bool {
        if key.ends_with('/') != self.folder {
            return false;
        }
        let parts: Vec<&str> = key.split('/').filter(|p| !p.is_empty()).collect();
        parts.len() == self.segments.len()
            && self
                .segments
                .iter()
                .zip(&parts)
                .all(|(glob, part)| segment_matches(glob, part))
    }

    /// The folder the design files live in: the glob's plain directory
    /// segments up to and including one named `design`, else all of them
    /// (`design` for design/mocks/*.dc.html, `web/design` for
    /// web/design/*.html). None when the glob has no plain directory.
    fn root(&self) -> Option<String> {
        let directories = self.segments.split_last()?.1;
        let plain: Vec<&String> = directories
            .iter()
            .take_while(|s| !s.contains(['*', '?']))
            .collect();
        let end = plain
            .iter()
            .position(|s| s.as_str() == "design")
            .map_or(plain.len(), |at| at + 1);
        (end > 0).then(|| {
            plain[..end]
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("/")
        })
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
    go(
        &glob.chars().collect::<Vec<_>>(),
        &text.chars().collect::<Vec<_>>(),
    )
}

/// The design files among <paths>, by <globs>. A source wins over a place
/// (the component sources sit in a folder the component glob matches).
pub fn classify(globs: &Globs, paths: &[String]) -> Changes {
    let parse = |list: &[String]| -> Vec<Glob> { list.iter().map(|g| Glob::parse(g)).collect() };
    let (places, sources, spec) = (
        parse(&globs.places),
        parse(&globs.sources),
        parse(&globs.spec),
    );
    let roots: BTreeSet<String> = places
        .iter()
        .chain(&sources)
        .filter_map(Glob::root)
        .collect();
    let mut changes = Changes::default();
    for path in paths {
        if sources.iter().any(|glob| glob.matches(path)) {
            changes.sources.insert(path.clone());
        } else if let Some(place) = places.iter().find_map(|glob| glob.place(path)) {
            changes.places.insert(place);
        } else if spec.iter().any(|glob| glob.matches(path)) {
            changes.spec_files.insert(path.clone());
        } else if roots.iter().any(|root| {
            path.strip_prefix(root.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        }) {
            changes.shared.insert(path.clone());
        } else {
            changes.outside.insert(path.clone());
        }
    }
    changes
}

/// The section of each line of <text>: the heading it sits under (the text
/// after the `#`s), None before the first heading. Headings inside a code
/// fence are not headings.
fn headings(text: &str) -> Vec<Option<String>> {
    let mut current: Option<String> = None;
    let mut fenced = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
                current = Some(line[hashes..].trim().to_string());
            }
        }
        out.push(current.clone());
    }
    out
}

fn section_key(path: &str, heading: &Option<String>) -> String {
    format!(
        "{path}#{}",
        heading.as_deref().unwrap_or("(before the first heading)")
    )
}

/// The new-side line ranges of a `-U0` diff's hunks: (first line, count).
fn hunks(diff: &str) -> Vec<(usize, usize)> {
    diff.lines()
        .filter_map(|line| {
            let header = line.strip_prefix("@@ ")?;
            let new = header
                .split_whitespace()
                .find_map(|part| part.strip_prefix('+'))?;
            let (start, count) = new.split_once(',').unwrap_or((new, "1"));
            Some((start.parse().ok()?, count.parse().ok()?))
        })
        .collect()
}

/// The sections of spec file <path> that commit <sha> touches. A file the
/// commit deletes is the one section `path#(file deleted)`; a deletion inside
/// a file touches the section the deleted lines sat in.
fn spec_sections(top: &Path, sha: &str, path: &str) -> Result<BTreeSet<String>> {
    let diff = git_text(
        git(top).args([
            "diff-tree",
            "-p",
            "-U0",
            "--no-color",
            "--no-commit-id",
            "--root",
            "-m",
            sha,
            "--",
            path,
        ]),
        &format!("read what {sha} changes in {path}"),
    )?;
    if diff.lines().any(|line| line == "+++ /dev/null") {
        return Ok(BTreeSet::from([format!("{path}#(file deleted)")]));
    }
    let text = git_text(
        git(top).args(["show", &format!("{sha}:{path}")]),
        &format!("read {path} as {sha} has it"),
    )?;
    let by_line = headings(&text);
    let mut sections = BTreeSet::new();
    for (start, count) in hunks(&diff) {
        let lines: Vec<usize> = if count == 0 {
            vec![start]
        } else {
            (start..start + count).collect()
        };
        for line in lines {
            let heading = line
                .checked_sub(1)
                .and_then(|at| by_line.get(at))
                .cloned()
                .flatten();
            sections.insert(section_key(path, &heading));
        }
    }
    Ok(sections)
}

/// The paths commit <sha> changes in the checkout at <top>. A merge counts
/// against each parent.
pub fn commit_paths(top: &Path, sha: &str) -> Result<Vec<String>> {
    let out = git_text(
        git(top).args([
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            "-m",
            "-z",
            sha,
            "--",
        ]),
        &format!("list the files {sha} changes"),
    )?;
    Ok(out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

/// What <shas> change among the design files of the repo whose globs are
/// <globs>.
pub fn changes_of(top: &Path, globs: &Globs, shas: &[String]) -> Result<Changes> {
    let mut paths = Vec::new();
    let mut per_commit = Vec::new();
    for sha in shas {
        let changed = commit_paths(top, sha)?;
        per_commit.push((sha, changed.clone()));
        paths.extend(changed);
    }
    let mut changes = classify(globs, &paths);
    if let Some(tip) = shas.last() {
        let boards: Vec<String> = changes
            .places
            .iter()
            .filter(|place| PlaceKind::of(place) == PlaceKind::Board)
            .cloned()
            .collect();
        for board in boards {
            if !exists_in(top, tip, &board)? {
                changes.places.remove(&board);
                changes.deleted.insert(board);
            }
        }
    }
    if changes.is_design_change() {
        for (sha, changed) in per_commit {
            for path in changed.iter().filter(|p| changes.spec_files.contains(*p)) {
                changes.spec.extend(spec_sections(top, sha, path)?);
            }
        }
    } else {
        changes.outside.append(&mut changes.spec_files);
    }
    Ok(changes)
}

/// How a commit draws some design paths: the git object each path is at
/// that commit (a board file's or a source's blob, a component folder's
/// tree), or None for a path the commit has no file or folder at. Two
/// drawings of a path are the same when their objects are, so the files are
/// byte-identical.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drawing {
    objects: BTreeMap<String, Option<String>>,
}

impl Drawing {
    /// Do this drawing and <other> both read <path>, and draw it the same?
    pub fn same(&self, other: &Drawing, path: &str) -> bool {
        match (self.objects.get(path), other.objects.get(path)) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => false,
        }
    }
}

/// How commit <rev> in the checkout at <top> draws <paths> (`Changes::drawn`
/// writes them). Fails when <rev> is no commit there. With no paths it reads
/// nothing.
pub fn drawing_at<'a>(
    top: &Path,
    rev: &str,
    paths: impl IntoIterator<Item = &'a String>,
) -> Result<Drawing> {
    let paths: Vec<&String> = paths.into_iter().collect();
    if paths.is_empty() {
        return Ok(Drawing::default());
    }
    // A commit that exists makes "missing" mean the path, never the commit.
    let commit = full_sha(top, rev)?;
    let asked: String = paths
        .iter()
        .map(|path| format!("{commit}:{}\n", path.trim_end_matches('/')))
        .collect();
    let what = format!("read the design paths at {rev}");
    let mut child = git(top)
        .args(["cat-file", "--batch-check=%(objectname)"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(|| format!("running git to {what}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    // git answers each line as it reads it, so the lines go in from another
    // thread while this one reads the answers.
    let output = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(asked.as_bytes()));
        let output = child.wait_with_output();
        let written = writer.join().expect("the writer does not panic");
        output.and_then(|output| written.map(|()| output))
    })
    .context(|| format!("running git to {what}"))?;
    if !output.status.success() {
        fail!(
            "git failed to {what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let answers = String::from_utf8_lossy(&output.stdout);
    let answers: Vec<&str> = answers.lines().collect();
    if answers.len() != paths.len() {
        fail!(
            "git answered {} of the {} design paths builds asked it to read at {rev}",
            answers.len(),
            paths.len()
        );
    }
    let objects = paths
        .into_iter()
        .zip(answers)
        .map(|(path, answer)| {
            let object = (!answer.ends_with(" missing")).then(|| answer.to_string());
            (path.clone(), object)
        })
        .collect();
    Ok(Drawing { objects })
}

/// Does <path> exist as a file or folder in commit <rev>?
pub fn exists_in(top: &Path, rev: &str, path: &str) -> Result<bool> {
    let output = git(top)
        .args(["cat-file", "-e", &format!("{rev}:{path}")])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| {
            crate::fail::Fail::Message(format!("running git to look for {path} in {rev}: {error}"))
        })?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(128) => Ok(false),
        _ => fail!(
            "git failed to look for {path} in {rev}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

/// The commits of <range> (`origin/main..HEAD`), oldest first; a range with
/// none is refused.
pub fn commits_of_range(top: &Path, range: &str) -> Result<Vec<String>> {
    let out = git_text(
        git(top).args(["rev-list", "--reverse", range, "--"]),
        &format!("list the commits of {range}"),
    )?;
    let shas: Vec<String> = out.lines().map(str::to_string).collect();
    if shas.is_empty() {
        fail!("the range {range} holds no commits");
    }
    Ok(shas)
}

/// The full hash of commit <sha>.
pub fn full_sha(top: &Path, sha: &str) -> Result<String> {
    let out = git_text(
        git(top).args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{sha}^{{commit}}"),
        ]),
        &format!("find commit {sha}"),
    )?;
    Ok(out.trim().to_string())
}

/// Is commit <sha> on origin/main, as the checkout at <top> knows it?
pub fn on_origin_main(top: &Path, sha: &str) -> Result<bool> {
    is_ancestor(top, sha, "origin/main")
}

/// Is commit <sha> the commit <rev> names or one before it, in the checkout
/// at <top>?
pub fn is_ancestor(top: &Path, sha: &str, rev: &str) -> Result<bool> {
    let status = git(top)
        .args(["merge-base", "--is-ancestor", sha, rev])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| {
            crate::fail::Fail::Message(format!("running git to test {sha} against {rev}: {error}"))
        })?;
    match status.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => fail!(
            "git failed to test {sha} against {rev}: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ),
    }
}

#[cfg(test)]
#[path = "design_paths_tests.rs"]
mod tests;
