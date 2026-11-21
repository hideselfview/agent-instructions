//! The run a run is compared with: another run's `findings.json`
//! (findings_file.rs), which `run --before` names. builds passes the file of
//! the run of a branch's merge base with origin/main, which it keeps, so a
//! branch's run fails only on the pairs it makes new or worse.
//!
//! A pair is new against the before run when that run did not check its
//! scene, and worse when it has a finding whose sentence the before run's
//! findings for the scene lack (each sentence counted as often as it
//! appears). A finding's sentence names its element by its role and words,
//! with no lengths, so the same finding on both commits has one sentence.
//! Warnings never fail a run, so only findings count.
//!
//! Findings made by another version of the checker would not compare, so a
//! file whose `version` is not this checker's is refused.

use crate::findings_file::Level;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Where a pair stands against the before run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Against {
    /// The before run did not check the scene.
    New,
    /// The pair has a finding the before run lacks for the scene.
    Worse,
    /// Every finding of the pair is among the before run's for the scene.
    Held,
}

/// The parts of a findings.json that the comparison reads.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: String,
    commit: String,
    scenes: Vec<String>,
    /// The pairs' sizes and the components' preview rows, which the
    /// comparison leaves unread.
    #[serde(rename = "sizes")]
    _sizes: serde::de::IgnoredAny,
    #[serde(rename = "components")]
    _components: serde::de::IgnoredAny,
    findings: Vec<Record>,
}

/// The parts of a record that the comparison reads; the file holds more.
#[derive(Deserialize)]
struct Record {
    scene: String,
    level: Level,
    sentence: String,
}

/// A before run: its commit, the scenes it checked and the sentences of each
/// scene's findings.
#[derive(Debug)]
pub(crate) struct Before {
    commit: String,
    scenes: BTreeSet<String>,
    findings: BTreeMap<String, Vec<String>>,
}

impl Before {
    /// The before run in <text>, the findings.json at <source>, which a
    /// checker of <version> must have written.
    pub(crate) fn parse(text: &str, source: &str, version: &str) -> Result<Self, String> {
        let file: File = serde_json::from_str(text)
            .map_err(|e| format!("{source} is not a shots run's findings.json: {e}"))?;
        if file.version != version {
            return Err(format!(
                "{source} holds the findings of shots checker version {}, and this is version {version}: a run compares only with a run of its own version",
                file.version
            ));
        }
        let mut findings: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for record in file.findings {
            if record.level == Level::Finding {
                findings
                    .entry(record.scene)
                    .or_default()
                    .push(record.sentence);
            }
        }
        Ok(Self {
            commit: file.commit,
            scenes: file.scenes.into_iter().collect(),
            findings,
        })
    }

    /// The before run in the findings.json at <path>, which the checker of
    /// <version> must have written.
    pub(crate) fn load(path: &Path, version: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::parse(&text, &path.display().to_string(), version)
    }

    /// The commit the before run checked.
    pub(crate) fn commit(&self) -> &str {
        &self.commit
    }

    /// Where the pair of <scene>, whose findings have <sentences>, stands.
    pub(crate) fn against(&self, scene: &str, sentences: &[String]) -> Against {
        if !self.scenes.contains(scene) {
            return Against::New;
        }
        let mut left: Vec<&String> = self
            .findings
            .get(scene)
            .map(|found| found.iter().collect())
            .unwrap_or_default();
        for sentence in sentences {
            match left.iter().position(|had| *had == sentence) {
                Some(at) => {
                    left.swap_remove(at);
                }
                None => return Against::Worse,
            }
        }
        Against::Held
    }
}

#[cfg(test)]
#[path = "before_tests.rs"]
mod tests;
