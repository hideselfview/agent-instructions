//! `builds shots findings ... --by-cause`: a cached run's findings grouped by
//! cause across every scene, so that an agent fixes a cause once instead of
//! one scene at a time.
//!
//! The shots program writes each finding and warning of a run as a record in
//! `findings.json`, beside its report (scripts/shots/src/findings_file.rs),
//! and builds caches the file with the report (shots_cache.rs). Each record
//! names the design-system component that draws the board element it is
//! about, when one does. This module reads the file and groups the records:
//! first by component, each with the kinds of finding it makes; the findings
//! of no component by their kind; then every record of a colour by its exact
//! colour pair (the board's colour and the shot's), or by the one colour that only one side draws, and every frame offset by
//! its offset to the nearest point. Each grouping ranks its causes by the
//! scenes they touch, then by their findings. The text names the ten scenes
//! where a cause makes the most findings, and counts the causes that make one
//! finding each instead of listing them.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::daemon::component_shots::Side;
use crate::fail;
use crate::fail::Result;
use crate::text::plural;

/// The file's name in the report's folder, as the shots program writes it.
pub const FILE: &str = "findings.json";

/// The parts of a run's findings.json that builds reads. The file also names
/// the program's version and the commit, which builds leaves unread.
#[derive(Debug, Deserialize)]
pub struct Run {
    /// Every scene the run checked.
    pub scenes: Vec<String>,
    pub findings: Vec<Record>,
}

/// One finding or warning of one scene.
#[derive(Debug, Deserialize)]
pub struct Record {
    pub scene: String,
    /// `finding`, which can fail a run, or `warning`, which never does.
    pub level: String,
    /// The sentence the shots program printed for the record.
    pub sentence: String,
    /// The design-system component that draws the board element the record
    /// is about; none when it is about no board element or no component
    /// draws it.
    #[serde(default)]
    pub component: Option<String>,
    #[serde(flatten)]
    pub what: What,
}

/// What a record says: its kind, with the fields of it that the grouping
/// reads. The file holds more (the element it names, its lengths and colour
/// differences), which builds leaves unread. A frame
/// offset is in points; a positive dx is right and a positive dy down, in the
/// shot against the board.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum What {
    Missing {},
    Extra {},
    OutOfOrder {},
    RoleDiffers {},
    StateDiffers {
        aspect: Aspect,
    },
    FrameOffset {
        dx: f64,
        dy: f64,
    },
    Moved {},
    Resized {},
    FontSize {},
    FontWeight {},
    Colour {
        part: Part,
        board: String,
        shot: String,
    },
    ForegroundColour {
        side: Side,
        colour: String,
    },
    TextColourUnavailable {},
    Region {},
}

/// Which state of an element differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Aspect {
    Value,
    Enabled,
    Selected,
}

/// Which colour of an element differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    Background,
    Text,
}

impl Part {
    fn word(self) -> &'static str {
        match self {
            Part::Background => "background",
            Part::Text => "text",
        }
    }
}

impl What {
    /// The finding's kind with its numbers and names taken out.
    fn kind(&self) -> &'static str {
        match self {
            What::Missing { .. } => "the shot lacks an element that the board has",
            What::Extra { .. } => "the shot has an element that the board lacks",
            What::OutOfOrder { .. } => "an element is in another place in the shot's reading order",
            What::RoleDiffers { .. } => "an element has another role in the shot",
            What::StateDiffers { aspect, .. } => match aspect {
                Aspect::Value => "an element has another value in the shot",
                Aspect::Enabled => "an element is enabled on one side and disabled on the other",
                Aspect::Selected => "an element is selected on one side and not on the other",
            },
            What::FrameOffset { .. } => "the shot's frame sits away from the board's",
            What::Moved { .. } => "an element moved in the shot",
            What::Resized { .. } => "an element has another size in the shot",
            What::FontSize { .. } => "a text has another font size in the shot",
            What::FontWeight { .. } => "a text has another font weight in the shot",
            What::Colour { part, .. } => match part {
                Part::Background => "an element has another background colour in the shot",
                Part::Text => "a text has another colour in the shot",
            },
            What::ForegroundColour { .. } => {
                "an element draws a foreground colour that the other side lacks"
            }
            What::TextColourUnavailable { .. } => "the checker could not measure a glyph colour",
            What::Region { .. } => "the pictures differ in an area",
        }
    }
}

/// One cause: the findings it makes and, for each scene it touches, how
/// many of them are there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub cause: String,
    pub findings: usize,
    /// Most findings first, then by name.
    pub scenes: Vec<(String, usize)>,
    /// The kinds of finding the cause makes, with how many of each, most
    /// first; empty for a cause that is one kind.
    pub kinds: Vec<(String, usize)>,
}

/// A run's findings grouped by cause, each grouping largest first.
#[derive(Debug, PartialEq, Eq)]
pub struct Causes {
    /// By the component that draws the board element.
    pub components: Vec<Group>,
    /// The findings no component draws, by their kind.
    pub remainder: Vec<Group>,
    /// The colour findings, by their exact colour pair.
    pub colours: Vec<Group>,
    /// The frame offsets, by their offset to the nearest point.
    pub frames: Vec<Group>,
}

/// Reads the findings.json text <text>.
pub fn parse(text: &str) -> Result<Run> {
    match serde_json::from_str(text) {
        Ok(run) => Ok(run),
        Err(error) => fail!("{FILE} is not what the shots program writes: {error}"),
    }
}

/// Reads the findings.json in the report folder <folder>.
pub fn read(folder: &Path) -> Result<Run> {
    let path = folder.join(FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(error) => fail!("cannot read {}: {error}", path.display()),
    }
}

/// The records of one cause, by scene and by kind.
#[derive(Default)]
struct Tally {
    findings: usize,
    scenes: BTreeMap<String, usize>,
    kinds: BTreeMap<String, usize>,
}

/// <counts> most first, then by name.
fn most_first(counts: BTreeMap<String, usize>) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
}

impl Tally {
    fn add(&mut self, scene: &str) {
        self.findings += 1;
        *self.scenes.entry(scene.to_string()).or_default() += 1;
    }

    fn add_kind(&mut self, scene: &str, kind: &str) {
        self.add(scene);
        *self.kinds.entry(kind.to_string()).or_default() += 1;
    }

    fn group(self, cause: String) -> Group {
        Group {
            cause,
            findings: self.findings,
            scenes: most_first(self.scenes),
            kinds: most_first(self.kinds),
        }
    }
}

/// <groups> largest first: most scenes, then most findings, then by cause.
fn ranked(groups: impl Iterator<Item = Group>) -> Vec<Group> {
    let mut groups: Vec<Group> = groups.collect();
    groups.sort_by(|a, b| {
        b.scenes
            .len()
            .cmp(&a.scenes.len())
            .then_with(|| b.findings.cmp(&a.findings))
            .then_with(|| a.cause.cmp(&b.cause))
    });
    groups
}

/// A frame offset as a key: whole points. The offsets of scenes that one
/// cause moves differ by fractions of a point (8, 7.5, 8.1), which no cause
/// tells apart.
fn points(length: f64) -> i64 {
    length.round() as i64
}

/// <run>'s findings grouped by cause.
pub fn causes(run: &Run) -> Causes {
    let mut components: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut remainder: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut colours: BTreeMap<String, Tally> = BTreeMap::new();
    let mut frames: BTreeMap<(i64, i64), Tally> = BTreeMap::new();
    for record in &run.findings {
        let scene = record.scene.as_str();
        let kind = record.what.kind();
        match &record.component {
            Some(component) => components
                .entry(component)
                .or_default()
                .add_kind(scene, kind),
            None => remainder.entry(kind).or_default().add(scene),
        }
        match &record.what {
            What::Colour { part, board, shot } => colours
                .entry(format!("{board} on the board is {shot} in the shot"))
                .or_default()
                .add_kind(scene, part.word()),
            What::ForegroundColour { side, colour } => colours
                .entry(match side {
                    Side::Board => format!("{colour} drawn on the board only"),
                    Side::Shot => format!("{colour} drawn in the shot only"),
                })
                .or_default()
                .add_kind(scene, "foreground"),
            What::FrameOffset { dx, dy } => frames
                .entry((points(*dx), points(*dy)))
                .or_default()
                .add(scene),
            _ => {}
        }
    }
    Causes {
        components: ranked(
            components
                .into_iter()
                .map(|(component, tally)| tally.group(component.to_string())),
        ),
        remainder: ranked(
            remainder
                .into_iter()
                .map(|(kind, tally)| tally.group(kind.to_string())),
        ),
        colours: ranked(
            colours
                .into_iter()
                .map(|(cause, tally)| tally.group(cause)),
        ),
        frames: ranked(frames.into_iter().map(|((dx, dy), tally)| {
            tally.group(format!(
                "the shot's frame sits {} pt {} and {} pt {} from the board's, to the nearest point",
                dx.abs(),
                if dx >= 0 { "right" } else { "left" },
                dy.abs(),
                if dy >= 0 { "down" } else { "up" },
            ))
        })),
    }
}

/// The most scenes a cause's line names: those where it makes the most
/// findings.
const SCENES_SHOWN: usize = 10;

/// <run>'s findings grouped by cause, as `builds shots findings --by-cause`
/// prints them.
pub fn text(run: &Run) -> String {
    let causes = causes(run);
    let scenes: std::collections::BTreeSet<&str> =
        run.findings.iter().map(|r| r.scene.as_str()).collect();
    let mut out = format!(
        "{} in {} of the {} the run checked, grouped by cause, largest first.\n",
        plural(run.findings.len(), "finding"),
        scenes.len(),
        plural(run.scenes.len(), "scene")
    );
    for (title, groups) in [
        ("By component", &causes.components),
        (
            "Findings that no component draws, by kind",
            &causes.remainder,
        ),
        ("By colour pair", &causes.colours),
        ("By frame offset", &causes.frames),
    ] {
        out.push('\n');
        if groups.is_empty() {
            out.push_str(&format!("{title}: none\n"));
            continue;
        }
        out.push_str(&format!("{title}:\n"));
        let (shared, single): (Vec<&Group>, Vec<&Group>) =
            groups.iter().partition(|group| group.findings > 1);
        for group in shared {
            out.push_str(&format!(
                "  {}: {}, {}\n",
                group.cause,
                plural(group.scenes.len(), "scene"),
                plural(group.findings, "finding")
            ));
            if !group.kinds.is_empty() {
                let kinds: Vec<String> = group
                    .kinds
                    .iter()
                    .map(|(kind, n)| format!("{n} {kind}"))
                    .collect();
                out.push_str(&format!("    kinds: {}\n", kinds.join("; ")));
            }
            let mut scenes: Vec<String> = group
                .scenes
                .iter()
                .take(SCENES_SHOWN)
                .map(|(scene, n)| format!("{scene} ({n})"))
                .collect();
            if group.scenes.len() > SCENES_SHOWN {
                scenes.push(format!("and {} more", group.scenes.len() - SCENES_SHOWN));
            }
            out.push_str(&format!("    scenes: {}\n", scenes.join(", ")));
        }
        if !single.is_empty() {
            out.push_str(&format!(
                "  and {} with one finding each\n",
                plural(single.len(), "cause")
            ));
        }
    }
    out
}

#[cfg(test)]
#[path = "shot_causes_tests.rs"]
mod tests;
