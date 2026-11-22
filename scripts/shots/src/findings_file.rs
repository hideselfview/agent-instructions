//! `findings.json`, written beside the report's `index.html`: each finding and
//! warning of the pairs a run checked, as a record with its kind and its
//! numbers and names in fields, so that a reader groups them without reading
//! the sentences. builds keeps the file with the report and groups it by cause
//! (`builds shots findings --by-cause`).
//!
//! The file is one object: `version`, the comparison version of the checker
//! that wrote it, and `capture_version`, the capture version of the checker
//! that made the pictures it compares (`builds shots version`); `commit`, the commit the run checked;
//! `scenes`, every pair the run checked; `seen`, the elements of each pair's
//! board and shot that the comparison read; `failed`, each selected scene the
//! capture wrote no usable shot of, with its `error`; `sizes`, each pair's board and shot
//! sizes in points, by scene, at which a reader shows the report's pictures
//! side by side; `components`, each design-system
//! component with a preview, once for each platform that draws it, with the
//! shots of its preview's rows in the preview's order, whether or not the run
//! checked them, so that a reader shows a component's rows and which of them
//! the run has a pair for (the dashboard's component page); and `findings`,
//! the records pair by pair, each pair's findings before its warnings. A later
//! run compares its pairs with the file (before.rs).
//!
//! A record holds the pair's name (`scene`), its `level` (`finding`, which
//! can fail a run, or `warning`), its `kind`, the sentence the run printed,
//! the design-system `component` that draws the board element it names
//! (absent when it names none, or no component draws it), and its kind's
//! fields: the `element` it names (its role and words), and lengths in points
//! rounded to a tenth as the sentence writes them. A positive `dx` is right and a positive `dy` is down, in the shot
//! against the board.

use crate::capture::Failed;
use crate::compare::colour::hex;
use crate::compare::normalize::Entry;
use crate::compare::{Aspect, Finding, Part, Warning};
use crate::component_files::Listing;
use crate::shots::{Role, Size};
use crate::verify::{Checked, Outcome};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The file's name in the report's folder.
pub(crate) const FILE: &str = "findings.json";

#[derive(Serialize)]
struct File<'a> {
    /// The version, commit, scenes and findings, which `recheck` prints alone.
    #[serde(flatten)]
    compared: Rechecked<'a>,
    failed: Vec<FailedScene>,
    /// How many problems the run found (a declared shot with no board, for
    /// one), each of which fails it. A recheck reads the count, since the
    /// problems come from the repo, which it does not read.
    problems: usize,
    sizes: BTreeMap<String, Sizes>,
    components: Vec<Component>,
}

/// A selected scene the capture wrote no usable shot of, and why. It is in no
/// pair, so a later run compares it as a scene the run did not check.
#[derive(Serialize, Deserialize)]
pub(crate) struct FailedScene {
    pub(crate) scene: String,
    pub(crate) error: String,
}

/// A pair's board and shot sizes, in points.
#[derive(Serialize)]
struct Sizes {
    board: Size,
    shot: Size,
}

/// A component on one platform, and the shot of each row of its preview.
#[derive(Serialize)]
struct Component {
    name: String,
    platform: &'static str,
    rows: Vec<String>,
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
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Element {
    pub(crate) role: Role,
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

/// A record's kind and the fields of its kind. A later run reads the
/// findings of a before run back as these (before.rs).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum What {
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
    TextColourUnavailable {
        element: Element,
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
            Warning::TextColourUnavailable { entry, .. } => Self::TextColourUnavailable {
                element: entry.into(),
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

/// The records of the findings and warnings of <checked>, pair by pair, each
/// pair's findings before its warnings.
fn records(checked: &[Checked]) -> Vec<Record> {
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
    findings
}

/// What `recheck` prints: the scenes it compared again and the records it made
/// of them, which are the parts of a findings.json that builds compares.
#[derive(Serialize)]
struct Rechecked<'a> {
    version: &'a str,
    capture_version: &'a str,
    commit: &'a str,
    scenes: Vec<String>,
    /// The elements each pair compared, by scene.
    seen: BTreeMap<String, Seen>,
    findings: Vec<Record>,
}

/// The elements of a pair's board and of its shot that the comparison read,
/// each role and words once. A later run's finding on an element that is in
/// neither list is that element's first observation (before.rs).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Seen {
    pub(crate) board: Vec<Element>,
    pub(crate) shot: Vec<Element>,
}

impl Seen {
    fn of(pair: &Checked) -> Self {
        let elements = |list: &crate::shots::ElementList| {
            let mut found: Vec<Element> = Vec::new();
            for entry in crate::compare::normalize::entries(&list.elements) {
                let element = Element::from(&entry);
                if !found.contains(&element) {
                    found.push(element);
                }
            }
            found
        };
        Self {
            board: elements(&pair.board),
            shot: elements(&pair.shot),
        }
    }
}

/// The text `recheck` prints for the pairs <checked>, which the checker of
/// comparison version <version> compared again from a run of <commit>, whose
/// pictures the checker of capture version <capture_version> made, with the
/// checker's verdict <passed> on the run.
pub(crate) fn rechecked(
    version: &str,
    capture_version: &str,
    commit: &str,
    checked: &[Checked],
    passed: bool,
) -> String {
    #[derive(Serialize)]
    struct Printed<'a> {
        #[serde(flatten)]
        compared: Rechecked<'a>,
        /// Whether the checker's verdict on the run is that nothing fails.
        passed: bool,
    }
    let compared = compared(version, capture_version, commit, checked);
    serde_json::to_string(&Printed { compared, passed }).expect("the records are plain data")
}

/// The scenes <checked> names and the records of their findings.
fn compared<'a>(
    version: &'a str,
    capture_version: &'a str,
    commit: &'a str,
    checked: &[Checked],
) -> Rechecked<'a> {
    Rechecked {
        version,
        capture_version,
        commit,
        scenes: checked.iter().map(|pair| pair.name.to_string()).collect(),
        seen: checked
            .iter()
            .map(|pair| (pair.name.to_string(), Seen::of(pair)))
            .collect(),
        findings: records(checked),
    }
}

/// The file's text for the pairs a run of <commit> by the checker of
/// comparison version <version> and capture version <capture_version> checked,
/// the scenes <failed>, and the components' preview rows that <listing> holds.
pub(crate) fn text(
    version: &str,
    capture_version: &str,
    commit: &str,
    checked: &[Checked],
    failed: &[Failed],
    listing: &Listing,
    problems: usize,
) -> String {
    let file = File {
        compared: compared(version, capture_version, commit, checked),
        problems,
        failed: failed
            .iter()
            .map(|f| FailedScene {
                scene: f.name.to_string(),
                error: f.why.clone(),
            })
            .collect(),
        sizes: checked
            .iter()
            .map(|pair| {
                let sizes = Sizes {
                    board: pair.board.size,
                    shot: pair.shot.size,
                };
                (pair.name.to_string(), sizes)
            })
            .collect(),
        components: listing
            .components
            .iter()
            .map(|component| Component {
                name: component.name.clone(),
                platform: component.platform.word(),
                rows: component
                    .rows
                    .iter()
                    .map(|row| row.shot.to_string())
                    .collect(),
            })
            .collect(),
    };
    serde_json::to_string(&file).expect("the records are plain data")
}

/// Writes the file for <outcome>, a run of <commit> by the checker of
/// comparison version <version> and capture version <capture_version>, into
/// the report's folder `dir`.
pub(crate) fn write(
    dir: &Path,
    version: &str,
    capture_version: &str,
    commit: &str,
    outcome: &Outcome,
) -> Result<(), String> {
    let path = dir.join(FILE);
    std::fs::write(
        &path,
        text(
            version,
            capture_version,
            commit,
            &outcome.checked,
            &outcome.failed,
            &outcome.components,
            outcome.problems.len(),
        ),
    )
    .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
#[path = "findings_file_tests.rs"]
mod tests;
