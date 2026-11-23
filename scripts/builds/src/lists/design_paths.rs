//! Which design places a set of commits changes: the changed paths from git,
//! matched against the globs of the repo's `design_places`.
//!
//! A glob is `/`-separated; a segment may hold `*` (any characters but `/`)
//! and `?` (one character), and a segment that is `**` alone matches any
//! number of segments, none included. A glob that ends in `/` names folders:
//! every path below a folder it matches belongs to that one place, counted
//! once, and the place is the folder with its trailing `/`. Any other glob
//! names files. `glob_fault` refuses the globs builds cannot match: `**`
//! beside other characters in a segment, and `**` in a glob of folders, whose
//! place would have no one depth.
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
use std::path::Path;

use crate::fail;
use crate::fail::{Context, Result};
use crate::glob::{Question, segments_match};
use crate::lists::Lists;
use boards::{git, git_text};

/// A repo's globs: its design places, its shared sources and its spec files,
/// and the folders of its screenshot capture files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Globs {
    pub places: Vec<String>,
    pub sources: Vec<String>,
    pub spec: Vec<String>,
    /// The repo's `shots-declared` folders (`boards::app_build`), relative to
    /// the repo's top: every file below one is a capture file. Empty until
    /// `with_capture_folders` reads them from a checkout.
    pub capture: Vec<String>,
}

/// A kind of file that only the rules marked for it judge: a design file by
/// `design: true`, a capture file by `capture: true`. A file of no kind is
/// code, which every rule whose paths match judges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// A board, a design-system file or canvas.json (`holds_design_file`).
    Design,
    /// A file under a `shots-declared` folder: it declares which views the
    /// shots capture, and the shots run checks it.
    Capture,
}

impl Globs {
    /// Whether the project's board or shared-source globs apply to the repo.
    pub(crate) fn has_design_files(lists: &Lists, repo: &str) -> bool {
        [&lists.design_places, &lists.design_sources]
            .iter()
            .any(|globs| globs.get(repo).is_some_and(|paths| !paths.is_empty()))
    }

    /// The globs of <repo> in <lists>.
    pub fn of(lists: &Lists, repo: &str) -> Globs {
        Globs::of_repo(
            repo,
            &lists.design_places,
            &lists.design_sources,
            &lists.spec_files,
        )
    }

    /// The globs of <repo> in the project's store <db> holds.
    pub fn read(db: &rusqlite::Connection, repo: &str) -> Result<Globs> {
        let project = crate::lists::project::read(db)?;
        Ok(Globs::of_repo(
            repo,
            &project.places,
            &project.sources,
            &project.specs,
        ))
    }

    /// The globs of <repo> among each repo's places, sources and spec files.
    fn of_repo(
        repo: &str,
        places: &BTreeMap<String, Vec<String>>,
        sources: &BTreeMap<String, Vec<String>>,
        spec: &BTreeMap<String, Vec<String>>,
    ) -> Globs {
        let get = |map: &BTreeMap<String, Vec<String>>| map.get(repo).cloned().unwrap_or_default();
        Globs {
            places: get(places),
            sources: get(sources),
            spec: get(spec),
            capture: Vec::new(),
        }
    }

    /// These globs with the capture folders that the `.app-build` of the
    /// checkout at <top> declares; none when it has no `.app-build`.
    pub fn with_capture_folders(mut self, top: &Path) -> Result<Globs> {
        let file = top.join(".app-build");
        self.capture = match std::fs::read_to_string(&file) {
            Ok(text) => boards::app_build::declared(&text, None)
                .map_err(|e| fail::Fail::Message(e.text()))?
                .into_iter()
                .map(|(_, folder)| folder)
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => fail!("reading {}: {error}", file.display()),
        };
        Ok(self)
    }

    /// The kind of file <path> is, or None for code. A design file wins over
    /// a capture file: no repo puts one under the other.
    pub fn kind_of(&self, path: &str) -> Option<FileKind> {
        if self.holds_design_file(path) {
            Some(FileKind::Design)
        } else if self.is_capture(path) {
            Some(FileKind::Capture)
        } else {
            None
        }
    }

    fn is_capture(&self, path: &str) -> bool {
        self.capture.iter().any(|folder| {
            path.strip_prefix(folder.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// Whether the change includes code, including code that captures screenshots.
    pub fn includes_code(&self, paths: &[String]) -> bool {
        !classify(self, paths).outside.is_empty()
    }

    /// Does <key> name a design place of these globs: a board file, or a
    /// component folder with its trailing `/`?
    pub fn names_place(&self, key: &str) -> bool {
        self.places.iter().any(|glob| Glob::parse(glob).names(key))
    }

    /// Is <path> a design file: a board or component file of a place, a
    /// shared design-system source, or another file under the folders the
    /// design files sit in, as `classify` counts it and `builds design only`
    /// accepts? A spec file is no design file.
    pub fn holds_design_file(&self, path: &str) -> bool {
        let changes = classify(self, &[path.to_string()]);
        changes.outside.is_empty() && changes.spec_files.is_empty()
    }

    /// Does <path> name one of these globs' spec files?
    pub fn names_spec_file(&self, path: &str) -> bool {
        self.spec.iter().any(|glob| Glob::parse(glob).matches(path))
    }

    /// The folders, relative to the repo's top, that hold every design place
    /// and shared source: the folder each place or source glob starts in
    /// (`Glob::root`), or the top itself ("") for a glob that starts with a
    /// wildcard.
    pub fn folders(&self) -> BTreeSet<String> {
        self.places
            .iter()
            .chain(&self.sources)
            .map(|glob| Glob::parse(glob).root().unwrap_or_default())
            .collect()
    }
}

/// What a place is, read from how its key is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    /// A board file.
    Board,
    /// The canvas index, `canvas.json`: it places the boards on the canvas
    /// and is no board itself, so it has no canvas link and no place on the
    /// review page.
    CanvasIndex,
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
        } else if key.rsplit('/').next() == Some("canvas.json") {
            PlaceKind::CanvasIndex
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
    /// Board files and component folders the commits delete: a place that is
    /// on origin/main and not at the last commit. It needs no answer, canvas
    /// link or place on the review page, and is not among `places`.
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
        if !self.folder {
            return segments_match(&self.segments, &parts, Question::Wildcard)
                .then(|| path.to_string());
        }
        // A glob of folders holds no `**` (`glob_fault`), so its place is the
        // path's first segments, one for each of the glob's.
        let n = self.segments.len();
        (parts.len() > n && segments_match(&self.segments, &parts[..n], Question::Wildcard))
            .then(|| format!("{}/", parts[..n].join("/")))
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
        segments_match(&self.segments, &parts, Question::Wildcard)
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

/// What is wrong with <glob>, which builds cannot match, or None: a segment
/// that holds `**` beside other characters, or `**` in a glob of folders.
pub fn glob_fault(glob: &str) -> Option<String> {
    let parsed = Glob::parse(glob);
    if parsed
        .segments
        .iter()
        .any(|segment| segment.contains("**") && segment != "**")
    {
        return Some(format!(
            "glob {glob} holds ** beside other characters in a segment; ** matches any number of folders only as a segment of its own"
        ));
    }
    if parsed.folder && parsed.segments.iter().any(|segment| segment == "**") {
        return Some(format!(
            "glob {glob} names folders and holds **, so a path could sit in folders of many depths and builds could not tell which one is the place; give each folder's depth with *"
        ));
    }
    None
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

/// The commit origin/main names in the checkout at <top>. Fails when there is
/// none, because git answers a missing ref and a missing path alike.
fn main_commit(top: &Path) -> Result<String> {
    let now = crate::design_render::NOW;
    match full_sha(top, now) {
        Ok(sha) if !sha.is_empty() => Ok(sha),
        _ => fail!(
            "the checkout at {} has no {now}, so it can't tell which boards the change deletes",
            top.display()
        ),
    }
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
        let drawn: Vec<String> = changes
            .places
            .iter()
            .filter(|place| {
                matches!(
                    PlaceKind::of(place),
                    PlaceKind::Board | PlaceKind::Component
                )
            })
            .cloned()
            .collect();
        let mut main: Option<String> = None;
        for place in drawn {
            // A component place is a folder with a trailing `/`.
            let path = place.trim_end_matches('/');
            if !boards::exists_in(top, tip, path)? {
                changes.places.remove(&place);
                // A board or component the range adds and deletes again was
                // never on origin/main, so the change deletes nothing.
                if main.is_none() {
                    main = Some(main_commit(top)?);
                }
                let on_main = main.as_deref().expect("resolved above");
                if boards::exists_in(top, on_main, path)? {
                    changes.deleted.insert(place);
                }
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
    let mut command = git(top);
    command.args(["cat-file", "--batch-check=%(objectname)"]);
    let (output, written) = crate::child_input::run(&mut command, asked.as_bytes())
        .context(|| format!("running git to {what}"))?;
    if !output.status.success() {
        fail!(
            "git failed to {what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    written.context(|| format!("writing to git to {what}"))?;
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
