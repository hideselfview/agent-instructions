//! `design/shots.toml`: the pairs checked against their boards. An entry
//! holds the hash of the board and of the shot (its element list and the
//! pixels of its picture) it was checked with, and the findings and warnings
//! the check gave. A pair whose two hashes equal its entry is skipped, and
//! its recorded findings stand in for a new check.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const FILE: &str = "design/shots.toml";

/// The record a run's results make, written beside its shots.
pub(crate) const RESULTS: &str = "record.toml";

const HEADER: &str = "# Written by `builds shots record`: the pairs of a shot and its board checked \
together, each with the hash of the board and of the shot, and the findings and warnings the \
check gave. Do not edit by hand.\n\n";

#[derive(Debug, thiserror::Error)]
pub(crate) enum RecordError {
    #[error("cannot read {0}: {1}")]
    Read(PathBuf, std::io::Error),
    #[error("cannot write {0}: {1}")]
    Write(PathBuf, std::io::Error),
    #[error("{0} is not a shot record: {1}")]
    Parse(String, toml::de::Error),
    #[error("cannot write the record: {0}")]
    Format(toml::ser::Error),
    #[error("git could not give {FILE} at {0}: {1}")]
    Git(String, String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    /// The board hash.
    pub(crate) board: String,
    /// The hash of the element list file and the picture's decoded pixels.
    pub(crate) shot: String,
    /// The findings the check gave the pair: the sentences of what the shot
    /// lacks or adds against the board. A pair with none agrees with its board.
    pub(crate) findings: Vec<String>,
    /// The warnings the check gave the pair.
    pub(crate) warnings: Vec<String>,
}

/// Where a pair stands against a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// The record's entry has both hashes.
    Unchanged,
    /// The record has an entry, with other hashes.
    Changed,
    /// The record has no entry.
    New,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Record {
    entries: BTreeMap<String, Entry>,
}

impl Record {
    pub(crate) fn parse(text: &str, source: &str) -> Result<Self, RecordError> {
        toml::from_str(text).map_err(|e| RecordError::Parse(source.to_string(), e))
    }

    /// The record in the file at `path`; none when there is no file.
    pub(crate) fn load(path: &Path) -> Result<Self, RecordError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, &path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(RecordError::Read(path.to_path_buf(), e)),
        }
    }

    /// The record of the branch `reference` in the repository at `root`; none
    /// when that commit holds no record.
    pub(crate) fn at(root: &Path, reference: &str) -> Result<Self, RecordError> {
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .map_err(|e| RecordError::Git(reference.to_string(), e.to_string()))
        };
        let commit = format!("{reference}^{{commit}}");
        let known = git(&["rev-parse", "--verify", "--quiet", &commit])?;
        if !known.status.success() {
            return Err(RecordError::Git(
                reference.to_string(),
                "no such commit".to_string(),
            ));
        }
        let object = format!("{reference}:{FILE}");
        if !git(&["cat-file", "-e", &object])?.status.success() {
            return Ok(Self::default());
        }
        let shown = git(&["show", &object])?;
        if !shown.status.success() {
            return Err(RecordError::Git(
                reference.to_string(),
                String::from_utf8_lossy(&shown.stderr).trim().to_string(),
            ));
        }
        Self::parse(&String::from_utf8_lossy(&shown.stdout), &object)
    }

    pub(crate) fn to_text(&self) -> Result<String, RecordError> {
        Ok(format!(
            "{HEADER}{}",
            toml::to_string(self).map_err(RecordError::Format)?
        ))
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), RecordError> {
        std::fs::write(path, self.to_text()?).map_err(|e| RecordError::Write(path.to_path_buf(), e))
    }

    /// Writes the record in `results` as the repository's record at `root`:
    /// the one place a run's results reach `design/shots.toml`.
    pub(crate) fn promote(results: &Path, root: &Path) -> Result<(), RecordError> {
        if !results.is_file() {
            return Err(RecordError::Read(
                results.to_path_buf(),
                std::io::Error::from(std::io::ErrorKind::NotFound),
            ));
        }
        Self::load(results)?.save(&root.join(FILE))
    }

    pub(crate) fn get(&self, board: &str) -> Option<&Entry> {
        self.entries.get(board)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    /// Records the check of the pair `board` as `entry`.
    pub(crate) fn set(&mut self, board: &str, entry: Entry) {
        self.entries.insert(board.to_string(), entry);
    }

    /// Where the pair of `board` with these hashes stands.
    pub(crate) fn standing(&self, board: &str, board_hash: &str, shot_hash: &str) -> Standing {
        match self.get(board) {
            None => Standing::New,
            Some(e) if e.board == board_hash && e.shot == shot_hash => Standing::Unchanged,
            Some(_) => Standing::Changed,
        }
    }
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
