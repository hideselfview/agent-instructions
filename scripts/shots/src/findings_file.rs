//! `findings.json`, written beside the report's `index.html`: each finding and
//! warning of the pairs a run checked, as a record with its kind and its
//! numbers and names in fields, so that a reader groups them without reading
//! the sentences. builds keeps the file with the report and groups it by cause
//! (`builds shots findings --by-cause`).
//!
//! The file is one object: `version`, the version of the program that wrote
//! it (`builds-shots version`); `commit`, the commit the run checked;
//! `scenes`, every pair the run checked; `boards`, each pair's board hash,
//! the hash of everything its board picture depends on, which keys the
//! picture in builds' picture folder; and `findings`, the records pair by
//! pair, each pair's findings before its warnings. A later run compares its
//! pairs with the file (before.rs).
//!
//! A record holds the pair's name (`scene`), its `level` (`finding`, which
//! can fail a run, or `warning`), its `kind`, the sentence the run printed,
//! the design-system `component` that draws the board element it names
//! (absent when it names none, or no component draws it), and its kind's
//! fields: the `element` it names (its role and words), and lengths in points
//! rounded to a tenth as the sentence writes them. A positive `dx` is right and a positive `dy` is down, in the shot
//! against the board.

use crate::compare::colour::hex;
use crate::compare::normalize::Entry;
use crate::compare::{Aspect, Finding, Part, Warning};
use crate::shots::Role;
use crate::verify::Checked;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The file's name in the report's folder.
pub(crate) const FILE: &str = "findings.json";

#[derive(Serialize)]
struct File<'a> {
    version: &'a str,
    commit: &'a str,
    scenes: Vec<String>,
    boards: BTreeMap<String, String>,
    findings: Vec<Record>,
}

/// Whether a record is a finding, which can fail a run, or a warning, which
/// never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Level {
    Finding,
    Warning,
}

#[derive(Serialize)]
struct Record {
    scene: String,
    level: Level,
    /// The design-system component that draws the board's element the record
    /// names (shots.rs); none when the record names no board element or no
    /// component draws it.
    #[serde(skip_serializing_if = "Option::is_none")]
    component: Option<String>,
    #[serde(flatten)]
    what: What,
    sentence: String,
}

/// An element as a record names it.
#[derive(Serialize)]
struct Element {
    role: Role,
    words: String,
}

impl From<&Entry> for Element {
    fn from(entry: &Entry) -> Self {
        Self {
            role: entry.kind,
            words: entry.words.clone(),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum What {
    Missing {
        element: Element,
    },
    Extra {
        element: Element,
    },
    OutOfOrder {
        element: Element,
    },
    /// `element` holds the board's role.
    RoleDiffers {
        element: Element,
        shot_role: Role,
    },
    StateDiffers {
        element: Element,
        aspect: Aspect,
        board: String,
        shot: String,
    },
    FrameOffset {
        dx: f64,
        dy: f64,
    },
    Moved {
        element: Element,
        dx: f64,
        dy: f64,
    },
    Resized {
        element: Element,
        dw: f64,
        dh: f64,
    },
    FontSize {
        element: Element,
        board: f64,
        shot: f64,
    },
    FontWeight {
        element: Element,
        board: u32,
        shot: u32,
    },
    Colour {
        element: Element,
        part: Part,
        board: String,
        shot: String,
        delta: f64,
    },
    Region {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        delta: f64,
    },
}

/// A length rounded to a tenth of a point, as a sentence writes it.
fn tenth(length: f64) -> f64 {
    let rounded = (length * 10.0).round() / 10.0;
    if rounded == 0.0 { 0.0 } else { rounded }
}

impl From<&Finding> for What {
    fn from(finding: &Finding) -> Self {
        match finding {
            Finding::Missing(entry) => Self::Missing {
                element: entry.into(),
            },
            Finding::Extra(entry) => Self::Extra {
                element: entry.into(),
            },
            Finding::OutOfOrder { board, .. } => Self::OutOfOrder {
                element: board.into(),
            },
            Finding::RoleDiffers { board, shot } => Self::RoleDiffers {
                element: board.into(),
                shot_role: shot.kind,
            },
            Finding::StateDiffers {
                board,
                shot,
                aspect,
            } => Self::StateDiffers {
                element: board.into(),
                aspect: *aspect,
                board: aspect.of(board),
                shot: aspect.of(shot),
            },
        }
    }
}

impl From<&Warning> for What {
    fn from(warning: &Warning) -> Self {
        match warning {
            Warning::FrameOffset { dx, dy } => Self::FrameOffset {
                dx: tenth(*dx),
                dy: tenth(*dy),
            },
            Warning::Moved { entry, dx, dy } => Self::Moved {
                element: entry.into(),
                dx: tenth(*dx),
                dy: tenth(*dy),
            },
            Warning::Resized { entry, dw, dh } => Self::Resized {
                element: entry.into(),
                dw: tenth(*dw),
                dh: tenth(*dh),
            },
            Warning::FontSize { entry, board, shot } => Self::FontSize {
                element: entry.into(),
                board: tenth(*board),
                shot: tenth(*shot),
            },
            Warning::FontWeight { entry, board, shot } => Self::FontWeight {
                element: entry.into(),
                board: *board,
                shot: *shot,
            },
            Warning::Colour {
                entry,
                part,
                board,
                shot,
                delta,
            } => Self::Colour {
                element: entry.into(),
                part: *part,
                board: hex(*board),
                shot: hex(*shot),
                delta: *delta,
            },
            Warning::Region { bounds, delta } => Self::Region {
                x: bounds.x,
                y: bounds.y,
                width: tenth(bounds.width),
                height: tenth(bounds.height),
                delta: *delta,
            },
        }
    }
}

/// The board's element a finding names; none for an element only the shot
/// has.
fn board_entry(finding: &Finding) -> Option<&Entry> {
    match finding {
        Finding::Missing(entry)
        | Finding::OutOfOrder { board: entry, .. }
        | Finding::RoleDiffers { board: entry, .. }
        | Finding::StateDiffers { board: entry, .. } => Some(entry),
        Finding::Extra(_) => None,
    }
}

/// The file's text for the pairs a run of <commit> checked.
pub(crate) fn text(commit: &str, checked: &[Checked]) -> String {
    let mut findings = Vec::new();
    for pair in checked {
        let scene = pair.name.to_string();
        let component = |entry: Option<&Entry>| {
            entry.and_then(|entry| pair.board.elements[entry.index].component.clone())
        };
        for finding in &pair.findings {
            findings.push(Record {
                scene: scene.clone(),
                level: Level::Finding,
                component: component(board_entry(finding)),
                what: finding.into(),
                sentence: finding.sentence(),
            });
        }
        for warning in &pair.warnings {
            findings.push(Record {
                scene: scene.clone(),
                level: Level::Warning,
                component: component(warning.entry()),
                what: warning.into(),
                sentence: warning.sentence(),
            });
        }
    }
    let file = File {
        version: crate::VERSION,
        commit,
        scenes: checked.iter().map(|pair| pair.name.to_string()).collect(),
        boards: checked
            .iter()
            .map(|pair| (pair.name.to_string(), pair.board_hash.clone()))
            .collect(),
        findings,
    };
    serde_json::to_string(&file).expect("the records are plain data")
}

/// Writes the file for the pairs a run of <commit> checked into the report's
/// folder `dir`.
pub(crate) fn write(dir: &Path, commit: &str, checked: &[Checked]) -> Result<(), String> {
    let path = dir.join(FILE);
    std::fs::write(&path, text(commit, checked))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
#[path = "findings_file_tests.rs"]
mod tests;
