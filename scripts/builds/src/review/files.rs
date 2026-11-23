//! A diff split by file, with the new text of each file around its hunks, for
//! the groups that review one file per call.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};

use boards::git;

/// Lines of the new file shown above and below each hunk.
const AROUND: i64 = 40;

/// Where the new text of a changed file is read: a revision of a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub repo: PathBuf,
    pub rev: String,
    /// Whether <repo> is a checkout of <rev> to read in place. When it is
    /// not, `checkout` unpacks <rev> into a folder of its own.
    pub in_place: bool,
}

/// A folder holding the repository as it is at the reviewed commit.
pub struct Checkout {
    pub path: PathBuf,
    owned: bool,
}

impl Drop for Checkout {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

impl Source {
    /// The checkout the reviewers read: the repo itself when it is one, else
    /// a copy of the files of <rev> (`git archive`) in a folder of its own,
    /// which the checkout deletes when it is dropped. Two copies of one
    /// commit get two folders: two commits with the same contents made in the
    /// same second get the same SHA, and dropping one copy deletes only its
    /// own folder.
    pub fn checkout(&self) -> Result<Checkout> {
        if self.in_place {
            return Ok(Checkout {
                path: self.repo.clone(),
                owned: false,
            });
        }
        let name = format!(
            "builds-review-{}-{}",
            crate::text::staged_suffix(),
            &self.rev
        );
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path).context(|| format!("creating {}", path.display()))?;
        let mut archive = git(&self.repo)
            .args(["archive", &self.rev])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context(|| format!("running git archive {}", self.rev))?;
        let tar = Command::new("tar")
            .arg("-x")
            .arg("-C")
            .arg(&path)
            .stdin(archive.stdout.take().expect("stdout is piped"))
            .output()
            .context(|| "running tar".to_string())?;
        let archived = archive
            .wait_with_output()
            .context(|| "waiting for git archive".to_string())?;
        if !archived.status.success() || !tar.status.success() {
            let _ = std::fs::remove_dir_all(&path);
            fail!(
                "unpacking {} of {} failed: {} {}",
                self.rev,
                self.repo.display(),
                String::from_utf8_lossy(&archived.stderr).trim(),
                String::from_utf8_lossy(&tar.stderr).trim()
            );
        }
        Ok(Checkout { path, owned: true })
    }
}

/// One file's part of a diff: its path in the new tree, its path in the old
/// tree when the diff renames it, and its diff text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub renamed_from: Option<String>,
    pub text: String,
}

const DIFF_HEADER: &str = "diff --git ";
const OLD_PATH_PREFIX: &str = "a/";
const NEW_PATH_PREFIX: &str = "b/";

/// Ask Git for the path prefixes that the patch parser reads.
pub(super) fn git_prefix_args() -> [String; 2] {
    [
        format!("--src-prefix={OLD_PATH_PREFIX}"),
        format!("--dst-prefix={NEW_PATH_PREFIX}"),
    ]
}

/// The destination in the `diff --git` line when no file header names it.
fn new_path(line: &str) -> Option<String> {
    let paths = line.strip_prefix(DIFF_HEADER)?;
    let paths = paths.strip_suffix('\n').unwrap_or(paths);
    if paths.ends_with('"') {
        let at = paths.rfind(&format!(" \"{NEW_PATH_PREFIX}"))?;
        return prefixed_path(&paths[at + 1..], NEW_PATH_PREFIX);
    }
    let mut candidates = paths.match_indices(' ').filter_map(|(at, _)| {
        let new = paths[at + 1..].strip_prefix(NEW_PATH_PREFIX)?;
        Some((&paths[..at], new))
    });
    let first = candidates.next()?;
    if candidates.clone().next().is_none() {
        return Some(first.1.to_string());
    }
    std::iter::once(first)
        .chain(candidates)
        .find(|(old, new)| old.strip_prefix(OLD_PATH_PREFIX) == Some(*new))
        .map(|(_, new)| new.to_string())
}

fn decode_path(path: &str) -> Option<String> {
    if !path.starts_with('"') {
        return Some(path.to_string());
    }
    let mut bytes = path.strip_prefix('"')?.strip_suffix('"')?.bytes();
    let mut decoded = Vec::new();
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'\\' {
            match bytes.next()? {
                escaped @ (b'"' | b'\\') => escaped,
                b'a' => 7,
                b'b' => 8,
                b't' => b'\t',
                b'n' => b'\n',
                b'v' => 11,
                b'f' => 12,
                b'r' => b'\r',
                first @ b'0'..=b'3' => {
                    let second = bytes.next().filter(|digit| (b'0'..=b'7').contains(digit))?;
                    let third = bytes.next().filter(|digit| (b'0'..=b'7').contains(digit))?;
                    (first - b'0') * 64 + (second - b'0') * 8 + (third - b'0')
                }
                _ => return None,
            }
        } else {
            byte
        };
        decoded.push(byte);
    }
    String::from_utf8(decoded).ok()
}

fn prefixed_path(path: &str, prefix: &str) -> Option<String> {
    let decoded = decode_path(path.strip_suffix('\t').unwrap_or(path))?;
    decoded.strip_prefix(prefix).map(str::to_string)
}

/// A file's destination path, and the source path if the diff renames it.
/// Git's file and rename headers give whole paths even when they contain spaces.
pub(super) fn file_paths(text: &str) -> Result<(String, Option<String>)> {
    decode_file_paths(text).ok_or_else(|| {
        let header = text.lines().next().unwrap_or_default();
        crate::fail::Fail::Message(format!(
            "Builds could not read a file path from the Git header {header:?}."
        ))
    })
}

fn decode_file_paths(text: &str) -> Option<(String, Option<String>)> {
    let mut lines = text.lines();
    let header = lines.next()?;
    let mut old = None;
    let mut renamed_from = None;
    let mut destination = None;
    for line in lines.take_while(|line| !line.starts_with("@@") && *line != "GIT binary patch") {
        if let Some(path) = line.strip_prefix("rename from ") {
            renamed_from = Some(decode_path(path)?);
        } else if let Some(path) = line.strip_prefix("rename to ") {
            destination = Some(decode_path(path)?);
        } else if destination.is_none() {
            if let Some(path) = line
                .strip_prefix("--- ")
                .filter(|path| *path != "/dev/null")
            {
                old = Some(prefixed_path(path, OLD_PATH_PREFIX)?);
            } else if let Some(path) = line.strip_prefix("+++ ") {
                destination = if path == "/dev/null" {
                    old.take()
                } else {
                    Some(prefixed_path(path, NEW_PATH_PREFIX)?)
                };
            }
        }
    }
    Some((destination.or_else(|| new_path(header))?, renamed_from))
}

pub(super) enum PatchPart<'a> {
    Text(&'a str),
    File(&'a str),
}

/// Whether a line starts one mail of `git format-patch` output.
pub(super) fn starts_mail(line: &str) -> bool {
    line.starts_with("From ")
        && line
            .split_whitespace()
            .nth(1)
            .is_some_and(|sha| sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The file diffs and the surrounding text, each in its original order.
pub(super) fn patch_parts(patch: &str) -> Vec<PatchPart<'_>> {
    let part = |text, file| {
        if file {
            PatchPart::File(text)
        } else {
            PatchPart::Text(text)
        }
    };
    let mut parts = Vec::new();
    let (mut start, mut at) = (0, 0);
    let mut file = false;
    for line in patch.split_inclusive('\n') {
        let starts_file = line.starts_with(DIFF_HEADER);
        if starts_file || starts_mail(line) {
            if start < at {
                parts.push(part(&patch[start..at], file));
            }
            start = at;
            file = starts_file;
        }
        at += line.len();
    }
    if start < at {
        parts.push(part(&patch[start..at], file));
    }
    parts
}

impl FileDiff {
    /// The paths this part changes: both paths of a rename, as
    /// `commits::paths` names them.
    pub fn paths(&self) -> Vec<String> {
        let mut paths = vec![self.path.clone()];
        paths.extend(self.renamed_from.clone());
        paths
    }
}

/// <diff> cut at each `diff --git` line, in order. Text before the first
/// such line (a patch's mail header) is dropped.
pub fn split(diff: &str) -> Result<Vec<FileDiff>> {
    let mut files = Vec::new();
    for part in patch_parts(diff) {
        if let PatchPart::File(text) = part {
            let (path, renamed_from) = file_paths(text)?;
            files.push(FileDiff {
                path,
                renamed_from,
                text: text.to_string(),
            });
        }
    }
    Ok(files)
}

/// Every path the files of <diff> change, each once, in order: both paths of
/// a rename.
pub fn paths(diff: &str) -> Result<Vec<String>> {
    let mut all: Vec<String> = Vec::new();
    for path in split(diff)?.iter().flat_map(FileDiff::paths) {
        if !all.contains(&path) {
            all.push(path);
        }
    }
    Ok(all)
}

/// The path the commit messages go by in a review: the reviewer reads them as
/// a file the change adds.
pub const COMMIT_MESSAGE: &str = "COMMIT_MESSAGE";

/// The commit messages as one added file, in diff form and as the quotes it
/// is checked against; None when there is no message text.
pub fn commit_message_file(messages: &str) -> Option<(FileDiff, crate::review::check::DiffFile)> {
    let lines: Vec<&str> = messages.trim().lines().collect();
    if lines.is_empty() {
        return None;
    }
    let mut text = format!(
        "diff --git a/{COMMIT_MESSAGE} b/{COMMIT_MESSAGE}\n--- /dev/null\n+++ b/{COMMIT_MESSAGE}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in &lines {
        text.push('+');
        text.push_str(line);
        text.push('\n');
    }
    let quotes = crate::review::check::parse_diff(&text)
        .expect("builds writes the commit messages as a valid patch")
        .pop()
        .expect("the commit messages' diff holds one file");
    Some((
        FileDiff {
            path: COMMIT_MESSAGE.to_string(),
            renamed_from: None,
            text,
        },
        quotes,
    ))
}

#[derive(Clone, Copy)]
pub struct LineRange {
    pub start: i64,
    pub count: i64,
}

impl LineRange {
    fn end(self) -> i64 {
        self.start + self.count.max(1)
    }
}

/// The old and new line ranges named by a Git hunk header.
pub fn hunk_ranges(line: &str) -> Option<(LineRange, LineRange)> {
    fn range(text: &str) -> Option<LineRange> {
        let (start, count) = text.split_once(',').unwrap_or((text, "1"));
        Some(LineRange {
            start: start.parse().ok()?,
            count: count.parse().ok()?,
        })
    }
    let mut parts = line.strip_prefix("@@ ")?.split_whitespace();
    let old = range(parts.next()?.strip_prefix('-')?)?;
    let new = range(parts.next()?.strip_prefix('+')?)?;
    Some((old, new))
}

/// Where a line from <from> sits in <head>, accounting for edits above it.
/// When Git replaces that line, returns the position in the replacement for
/// the caller to check against the quote.
pub fn line_at(repo: &Path, from: &str, head: &str, path: &str, line: i64) -> Result<Option<i64>> {
    if from == head {
        return Ok(Some(line));
    }
    let diff = super::commits::git_text(
        repo,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--unified=0",
            "--inter-hunk-context=0",
            from,
            head,
            "--",
            &format!(":(top,literal){path}"),
        ],
    )?;
    let mut offset = 0;
    for (old, new) in diff.lines().filter_map(hunk_ranges) {
        if line < old.start || (old.count == 0 && line == old.start) {
            break;
        }
        if line < old.end() {
            let within = line - old.start;
            return Ok((within < new.count).then_some(new.start + within));
        }
        offset = new.end() - old.end();
    }
    Ok(Some(line + offset))
}

/// The new text of <path> around each hunk of <file_diff>, every line
/// numbered, as the reviewer reads it; empty when the text is not available.
pub fn context(source: Option<&Source>, path: &str, file_diff: &str) -> String {
    let Some(source) = source else {
        return String::new();
    };
    let Some(text) = new_text(&source.repo, &source.rev, path) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len() as i64;
    let mut ranges: Vec<(i64, i64)> = Vec::new();
    for (_, new) in file_diff.lines().filter_map(hunk_ranges) {
        let from = (new.start - AROUND).max(1);
        let to = (new.end() - 1 + AROUND).min(total);
        match ranges.last_mut() {
            Some(last) if from <= last.1 + 1 => last.1 = last.1.max(to),
            _ => ranges.push((from, to)),
        }
    }
    let mut out = String::new();
    for (from, to) in ranges {
        if from > to {
            continue;
        }
        out.push_str(&format!("lines {from} to {to}:\n"));
        for n in from..=to {
            out.push_str(&format!("{n}: {}\n", lines[(n - 1) as usize]));
        }
        out.push('\n');
    }
    out
}

fn new_text(repo: &Path, rev: &str, path: &str) -> Option<String> {
    let output = git(repo)
        .args(["show", &format!("{rev}:{path}")])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
