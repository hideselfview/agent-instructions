//! The run a run is compared with: another run's `findings.json`
//! (findings_file.rs), which `run --before` names. builds passes the file of
//! the run of a branch's merge base with origin/main, which it keeps, so a
//! branch's run fails only on the pairs it makes new or worse.
//!
//! A pair is new against the before run when that run did not check its
//! scene, and worse when it has a finding that no finding of the before run
//! for the scene covers. A finding covers another when both are of one kind
//! about one element (its role and words), and for a state, about one aspect,
//! and the shot is at least as far from the board in it as in the other
//! (`Size`). Each before finding covers one finding of the pair. So a value
//! that moved closer to the board's is no new finding, and one that moved
//! away is.
//! Warnings never fail a run, so only findings count.
//!
//! Findings made by another version of the checker would not compare, so a
//! file whose `version` is not this checker's is refused.

use crate::compare::Aspect;
use crate::findings_file::{Level, What};
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

/// The parts of a findings.json that the comparison reads. A scene the run
/// failed to capture is in `failed` and not in `scenes`, so it reads as a
/// scene the run did not check.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: String,
    commit: String,
    scenes: Vec<String>,
    /// The pairs' sizes and the components' preview rows, which the
    /// comparison leaves unread.
    #[serde(rename = "failed")]
    _failed: serde::de::IgnoredAny,
    #[serde(rename = "sizes")]
    _sizes: serde::de::IgnoredAny,
    #[serde(rename = "components")]
    _components: serde::de::IgnoredAny,
    findings: Vec<Record>,
}

/// The parts of a record that the comparison reads: its kind and the fields
/// of its kind, and not its sentence or component.
#[derive(Deserialize)]
struct Record {
    scene: String,
    level: Level,
    #[serde(flatten)]
    what: What,
}

/// A before run: its commit, the scenes it checked and each scene's findings.
#[derive(Debug)]
pub(crate) struct Before {
    commit: String,
    scenes: BTreeSet<String>,
    findings: BTreeMap<String, Vec<What>>,
}

/// How far the shot is from the board in a finding. A difference between two
/// numbers (a slider's value) is measured; any other difference (an element
/// the shot lacks, a toggle that is off, a field's other text) is not, and
/// counts as farther than every measured one, so a measured finding never
/// covers it.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
enum Size {
    /// The distance between the board's number and the shot's.
    Measured(f64),
    Unmeasured,
}

impl What {
    /// Whether <self> and <other> are of one kind about one element, and for
    /// a state, about one aspect. A role difference is about the board's
    /// element, whatever role the shot reads it as. The records of warnings
    /// never reach the comparison, so they are the same as nothing.
    fn same_finding(&self, other: &What) -> bool {
        match (self, other) {
            (Self::Missing { element: a }, Self::Missing { element: b })
            | (Self::Extra { element: a }, Self::Extra { element: b })
            | (Self::OutOfOrder { element: a }, Self::OutOfOrder { element: b })
            | (Self::RoleDiffers { element: a, .. }, Self::RoleDiffers { element: b, .. }) => {
                a == b
            }
            (
                Self::StateDiffers {
                    element: a,
                    aspect: x,
                    ..
                },
                Self::StateDiffers {
                    element: b,
                    aspect: y,
                    ..
                },
            ) => a == b && x == y,
            _ => false,
        }
    }

    /// How far the shot is from the board in the finding.
    fn size(&self) -> Size {
        let number = |text: &str| text.parse::<f64>().ok().filter(|n| n.is_finite());
        match self {
            Self::StateDiffers {
                aspect: Aspect::Value,
                board,
                shot,
                ..
            } => match (number(board), number(shot)) {
                (Some(board), Some(shot)) => Size::Measured((shot - board).abs()),
                _ => Size::Unmeasured,
            },
            _ => Size::Unmeasured,
        }
    }

    /// Whether <self>, a before run's finding, covers <finding>: the same
    /// finding, with the shot at least as far from the board.
    fn covers(&self, finding: &What) -> bool {
        self.same_finding(finding) && finding.size() <= self.size()
    }
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
        let mut findings: BTreeMap<String, Vec<What>> = BTreeMap::new();
        for record in file.findings {
            if record.level == Level::Finding {
                findings.entry(record.scene).or_default().push(record.what);
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

    /// Where the pair of <scene>, whose findings are <findings>, stands.
    /// The pair's findings take before findings farthest first: a before
    /// finding that covers one finding covers every nearer one of the same
    /// element, so which of them it takes does not change the outcome.
    pub(crate) fn against(&self, scene: &str, findings: &[What]) -> Against {
        if !self.scenes.contains(scene) {
            return Against::New;
        }
        let mut left: Vec<&What> = self
            .findings
            .get(scene)
            .map(|found| found.iter().collect())
            .unwrap_or_default();
        let mut farthest_first: Vec<&What> = findings.iter().collect();
        farthest_first.sort_by(|a, b| b.size().partial_cmp(&a.size()).expect("sizes are finite"));
        for finding in farthest_first {
            match left.iter().position(|had| had.covers(finding)) {
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
