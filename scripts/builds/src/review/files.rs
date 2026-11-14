//! A diff split by file, with the new text of each file around its hunks, for
//! the groups that review one file per call.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::checkout::git;

/// Lines of the new file shown above and below each hunk.
const AROUND: i64 = 40;

/// Where the new text of a changed file is read: a revision of a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub repo: PathBuf,
    pub rev: String,
}

/// One file's part of a diff: its path in the new tree and its diff text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub text: String,
}

/// <diff> cut at each `diff --git` line, in order. Text before the first
/// such line (a patch's mail header) is dropped.
pub fn split(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    for line in diff.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let rest = rest.trim_end();
            let path = rest.rsplit_once(" b/").map(|(_, new)| new).unwrap_or(rest);
            files.push(FileDiff {
                path: path.to_string(),
                text: String::new(),
            });
        }
        if let Some(file) = files.last_mut() {
            file.text.push_str(line);
        }
    }
    files
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
