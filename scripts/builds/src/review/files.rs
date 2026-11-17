//! A diff split by file, with the new text of each file around its hunks, for
//! the groups that review one file per call.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};

use crate::checkout::git;

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
pub fn split(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    for line in diff.split_inclusive('\n') {
        if let Some(path) = crate::review::check::new_path(line) {
            files.push(FileDiff {
                path: path.to_string(),
                renamed_from: None,
                text: String::new(),
            });
        }
        if let Some(file) = files.last_mut() {
            // Only a header line starts with `rename from `: each hunk line
            // starts with its `+`, `-` or space mark.
            if let Some(old) = line.strip_prefix("rename from ") {
                file.renamed_from = Some(old.trim_end().to_string());
            }
            file.text.push_str(line);
        }
    }
    files
}

/// Every path the files of <diff> change, each once, in order: both paths of
/// a rename.
pub fn paths(diff: &str) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for path in split(diff).iter().flat_map(FileDiff::paths) {
        if !all.contains(&path) {
            all.push(path);
        }
    }
    all
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
    Some((
        FileDiff {
            path: COMMIT_MESSAGE.to_string(),
            renamed_from: None,
            text,
        },
        crate::review::check::DiffFile {
            path: COMMIT_MESSAGE.to_string(),
            lines: lines.iter().map(|l| l.to_string()).collect(),
        },
    ))
}

/// The new-file line ranges (first line, line count) of <diff>'s hunks.
fn hunks(diff: &str) -> Vec<(i64, i64)> {
    diff.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("@@ ")?;
            let new = rest.split_whitespace().find(|part| part.starts_with('+'))?;
            let (start, count) = match new[1..].split_once(',') {
                Some((start, count)) => (start.parse().ok()?, count.parse().ok()?),
                None => (new[1..].parse().ok()?, 1),
            };
            Some((start, count))
        })
        .collect()
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
    for (start, count) in hunks(file_diff) {
        let from = (start - AROUND).max(1);
        let to = (start + count.max(1) - 1 + AROUND).min(total);
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
