//! Comparing a board's element list with a shot's. The hard checks live here:
//! what the shot lacks, what it adds, what it reads in another place, as
//! another kind, or in another state.

pub(crate) mod align;
pub(crate) mod colour;
pub(crate) mod geometry;
pub(crate) mod normalize;
pub(crate) mod regions;

use crate::shots::{Bounds, Element, Picture, Role, Size};
use align::Step;
use colour::{Rgb, hex};
use normalize::{Entry, entries, fold_merged};
use serde::{Deserialize, Serialize};

/// A box edge may differ by this many points before it warns.
pub(crate) const EDGE_TOLERANCE: f64 = 2.0;
/// A font size may differ by this many points before it warns.
pub(crate) const FONT_SIZE_TOLERANCE: f64 = 0.5;
/// A CIEDE2000 difference past this warns.
pub(crate) const DELTA_E_TOLERANCE: f64 = 3.0;
/// The side of a cell of the region grid, in points.
pub(crate) const REGION_CELL: f64 = 16.0;

/// The tolerances, named, for the report to print.
pub(crate) fn tolerances() -> Vec<String> {
    vec![
        format!("box edges {EDGE_TOLERANCE} pt"),
        format!("font size {FONT_SIZE_TOLERANCE} pt"),
        format!("colour ΔE {DELTA_E_TOLERANCE}"),
        format!(
            "foreground at least ΔE {} from the background",
            colour::FAR_FROM_BACKGROUND
        ),
        format!("region cells {REGION_CELL} pt"),
    ]
}

/// What differs between a matched board element and shot element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Aspect {
    Value,
    Enabled,
    Selected,
}

impl Aspect {
    /// The state of <entry> in this aspect: its value, `enabled` or
    /// `disabled`, `selected` or `not selected`.
    pub(crate) fn of(self, entry: &Entry) -> String {
        let word = match self {
            Self::Value => return entry.value.clone(),
            Self::Enabled if entry.enabled => "enabled",
            Self::Enabled => "disabled",
            Self::Selected if entry.selected => "selected",
            Self::Selected => "not selected",
        };
        word.to_string()
    }
}

/// A reason the run fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Finding {
    /// The board has it, the shot does not.
    Missing(Entry),
    /// The shot has it, the board does not.
    Extra(Entry),
    /// Both have it, in another place of the reading order.
    OutOfOrder { board: Entry, shot: Entry },
    /// The same words at the same place, read as another kind.
    RoleDiffers { board: Entry, shot: Entry },
    StateDiffers {
        board: Entry,
        shot: Entry,
        aspect: Aspect,
    },
}

/// The outcome of comparing two lists.
#[derive(Debug)]
pub(crate) struct Comparison {
    /// The elements both lists hold at the same place: (board, shot).
    pub(crate) pairs: Vec<(Entry, Entry)>,
    /// In the order the elements come in the reading order.
    pub(crate) findings: Vec<Finding>,
}

/// The board's preview slots: the boxes of its `bae-preview-slot` stand-ins,
/// where the app draws its real content, and the board's frame. The real view
/// can run past the frame (a pane that scrolls) and the shot lists what runs
/// past, so a slot that reaches the frame's right or bottom edge goes on past
/// it. This is the one place that decides what a slot covers.
#[derive(Clone, Copy)]
pub(crate) struct Slots<'a> {
    boxes: &'a [Bounds],
    frame: Size,
}

impl<'a> Slots<'a> {
    /// A board with no slots, for tests.
    #[cfg(test)]
    pub(crate) const NONE: Slots<'static> = Slots {
        boxes: &[],
        frame: Size {
            width: 0.0,
            height: 0.0,
        },
    };

    pub(crate) fn new(boxes: &'a [Bounds], frame: Size) -> Self {
        Self { boxes, frame }
    }

    /// The areas the slots cover in the board's frame.
    pub(crate) fn covered(&self) -> Vec<Bounds> {
        let past = |start: f64, len: f64, edge: f64| {
            if start + len >= edge - EDGE_TOLERANCE {
                f64::from(f32::MAX)
            } else {
                len
            }
        };
        self.boxes
            .iter()
            .map(|b| Bounds {
                width: past(b.x, b.width, self.frame.width),
                height: past(b.y, b.height, self.frame.height),
                ..*b
            })
            .collect()
    }
}

/// Compares the board's list with the shot's, leaving out what either draws
/// inside a preview slot of the board. An element is inside a slot when the
/// middle of its box lies in the slot's; that drops the slot's own element
/// too. The shot's frame may sit off the board's, so the shot's slots are the
/// board's moved by the offset the pairs outside the slots give. This is the
/// one place slots filter the element comparison.
pub(crate) fn compare<'a>(boards: &'a [Element], shot: &'a [Element], slots: Slots) -> Comparison {
    let covered = slots.covered();
    let outside = |list: &'a [Element], shift: (f64, f64)| {
        let slots: Vec<Bounds> = covered
            .iter()
            .map(|s| Bounds {
                x: s.x + shift.0,
                y: s.y + shift.1,
                ..*s
            })
            .collect();
        move |entry: &Entry| {
            let at = entry.bounds(list);
            !slots.iter().any(|s| within(at, *s))
        }
    };
    if covered.is_empty() {
        return compare_kept(boards, shot, |_| true, |_| true);
    }
    let rough = compare_kept(boards, shot, outside(boards, (0.0, 0.0)), |_| true);
    let offset = geometry::frame_offset(boards, shot, &rough.pairs);
    compare_kept(
        boards,
        shot,
        outside(boards, (0.0, 0.0)),
        outside(shot, offset),
    )
}

/// Whether the middle of `inner` lies within `outer`. A shot's real content
/// can reach past the slot's box (a pane taller than the board's frame), so
/// the middle decides, not the whole box.
fn within(inner: Bounds, outer: Bounds) -> bool {
    let (x, y) = (inner.x + inner.width / 2.0, inner.y + inner.height / 2.0);
    x >= outer.x && x <= outer.x + outer.width && y >= outer.y && y <= outer.y + outer.height
}

/// Compares the entries of the two lists that `keep_board` and `keep_shot`
/// keep.
fn compare_kept(
    boards: &[Element],
    shot: &[Element],
    keep_board: impl Fn(&Entry) -> bool,
    keep_shot: impl Fn(&Entry) -> bool,
) -> Comparison {
    let mut shot = entries(shot);
    shot.retain(&keep_shot);
    let mut board = entries(boards);
    board.retain(&keep_board);
    let board = fold_merged(board, &shot);
    let in_dialog: Vec<bool> = board
        .iter()
        .map(|e| inside_dialog(boards, e.index))
        .collect();
    let steps = align_by_window(&board, &in_dialog, &shot);

    // Findings with their step number, which orders them.
    let mut pairs = Vec::new();
    let mut found: Vec<(usize, Finding)> = Vec::new();
    // Leftovers: (step, gap, entry). A gap is a run of steps between matches.
    let mut missing: Vec<(usize, usize, &Entry)> = Vec::new();
    let mut extra: Vec<(usize, usize, &Entry)> = Vec::new();
    let mut gap = 0;
    for (at, step) in steps.iter().enumerate() {
        match *step {
            Step::Match(b, s) => {
                gap += 1;
                pairs.push((board[b].clone(), shot[s].clone()));
                for aspect in differences(&board[b], &shot[s]) {
                    found.push((
                        at,
                        Finding::StateDiffers {
                            board: board[b].clone(),
                            shot: shot[s].clone(),
                            aspect,
                        },
                    ));
                }
            }
            Step::Board(b) => missing.push((at, gap, &board[b])),
            Step::Shot(s) => extra.push((at, gap, &shot[s])),
        }
    }

    // The same kind and words left on both sides: one element out of order.
    let mut taken = vec![false; extra.len()];
    let mut missing_left = Vec::new();
    for &(at, gap, entry) in &missing {
        let same = extra
            .iter()
            .enumerate()
            .find(|(i, e)| !taken[*i] && e.2.key() == entry.key());
        match same {
            Some((i, other)) => {
                taken[i] = true;
                found.push((
                    at.min(other.0),
                    Finding::OutOfOrder {
                        board: entry.clone(),
                        shot: other.2.clone(),
                    },
                ));
            }
            None => missing_left.push((at, gap, entry)),
        }
    }
    // The same words in the same gap, another kind: one role difference.
    let mut extra_left: Vec<_> = extra
        .iter()
        .enumerate()
        .filter(|(i, _)| !taken[*i])
        .map(|(_, e)| *e)
        .collect();
    for (at, gap, entry) in missing_left {
        let same = extra_left
            .iter()
            .position(|e| e.1 == gap && !entry.words.is_empty() && e.2.words == entry.words);
        match same {
            Some(i) => {
                let other = extra_left.remove(i);
                found.push((
                    at.min(other.0),
                    Finding::RoleDiffers {
                        board: entry.clone(),
                        shot: other.2.clone(),
                    },
                ));
            }
            None => found.push((at, Finding::Missing(entry.clone()))),
        }
    }
    for (at, _, entry) in extra_left {
        found.push((at, Finding::Extra(entry.clone())));
    }
    found.sort_by_key(|(at, _)| *at);
    Comparison {
        pairs,
        findings: found.into_iter().map(|(_, f)| f).collect(),
    }
}

/// Whether the board's element `index` sits inside a dialog or alert.
fn inside_dialog(board: &[Element], index: usize) -> bool {
    let mut at = Some(index);
    while let Some(i) = at {
        if matches!(board[i].native.as_str(), "dialog" | "alertdialog") {
            return true;
        }
        at = board[i].parent.filter(|p| *p < i);
    }
    false
}

/// The alignment of the board's entries with the shot's, comparing reading
/// order inside the window and inside the dialog, never between them: the
/// platform lists the front dialog before the window behind it, and the board
/// lists it last. The window's entries are aligned with the whole shot, then
/// the dialog's with the shot elements the window did not take.
fn align_by_window(board: &[Entry], in_dialog: &[bool], shot: &[Entry]) -> Vec<Step> {
    let window: Vec<usize> = (0..board.len()).filter(|i| !in_dialog[*i]).collect();
    let dialog: Vec<usize> = (0..board.len()).filter(|i| in_dialog[*i]).collect();
    let keys = |entries: &[Entry], at: &[usize]| -> Vec<_> {
        at.iter().map(|i| entries[*i].key()).collect()
    };
    let all_shot: Vec<usize> = (0..shot.len()).collect();
    let steps = align::align(&keys(board, &window), &keys(shot, &all_shot));
    let steps: Vec<Step> = steps
        .into_iter()
        .map(|step| match step {
            Step::Match(b, s) => Step::Match(window[b], s),
            Step::Board(b) => Step::Board(window[b]),
            Step::Shot(s) => Step::Shot(s),
        })
        .collect();
    if dialog.is_empty() {
        return steps;
    }
    // The shot elements the window left over, and the dialog aligned to them.
    let left: Vec<usize> = steps
        .iter()
        .filter_map(|step| {
            if let Step::Shot(s) = step {
                Some(*s)
            } else {
                None
            }
        })
        .collect();
    let inner: Vec<Step> = align::align(&keys(board, &dialog), &keys(shot, &left))
        .into_iter()
        .map(|step| match step {
            Step::Match(b, s) => Step::Match(dialog[b], left[s]),
            Step::Board(b) => Step::Board(dialog[b]),
            Step::Shot(s) => Step::Shot(left[s]),
        })
        .collect();
    // A dialog entry the shot lacks goes before the next shot element of the
    // dialog's alignment, or last.
    let mut matched = std::collections::HashMap::new();
    let mut before: std::collections::HashMap<usize, Vec<usize>> = Default::default();
    let mut last = Vec::new();
    let mut pending = Vec::new();
    for step in &inner {
        match *step {
            Step::Board(b) => pending.push(b),
            Step::Match(b, s) => {
                matched.insert(s, b);
                before.entry(s).or_default().append(&mut pending);
            }
            Step::Shot(s) => before.entry(s).or_default().append(&mut pending),
        }
    }
    last.append(&mut pending);
    let mut out = Vec::with_capacity(steps.len() + dialog.len());
    for step in steps {
        match step {
            Step::Shot(s) => {
                out.extend(before.remove(&s).into_iter().flatten().map(Step::Board));
                out.push(match matched.get(&s) {
                    Some(b) => Step::Match(*b, s),
                    None => Step::Shot(s),
                });
            }
            other => out.push(other),
        }
    }
    out.extend(last.into_iter().map(Step::Board));
    out
}

/// One side of a comparison: its elements and its picture, which is `scale`
/// pixels to the point.
pub(crate) struct Side<'a> {
    pub(crate) elements: &'a [Element],
    pub(crate) picture: &'a Picture,
    pub(crate) scale: f64,
    /// Areas this side's picture draws that the other side's does not, in
    /// the side's own frame (the board's on-screen keyboard); the picture
    /// check leaves them out.
    pub(crate) unshot: &'a [Bounds],
}

/// Which colour of an element differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Part {
    Background,
    Foreground,
    Text,
}

/// A difference that is reported and never fails the run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Warning {
    /// The shot's frame sits this far right and down of the board's.
    FrameOffset {
        dx: f64,
        dy: f64,
    },
    /// In the shot the element sits this far right and down of the board's.
    Moved {
        entry: Entry,
        dx: f64,
        dy: f64,
    },
    /// In the shot the element is this much wider and taller.
    Resized {
        entry: Entry,
        dw: f64,
        dh: f64,
    },
    FontSize {
        entry: Entry,
        board: f64,
        shot: f64,
    },
    FontWeight {
        entry: Entry,
        board: u32,
        shot: u32,
    },
    Colour {
        entry: Entry,
        part: Part,
        board: Rgb,
        shot: Rgb,
        delta: f64,
    },
    /// A region of the board's frame where the pictures differ.
    Region {
        bounds: Bounds,
        delta: f64,
    },
}

/// The warnings for the matched `pairs` of two sides.
pub(crate) fn warnings(board: &Side, shot: &Side, pairs: &[(Entry, Entry)]) -> Vec<Warning> {
    let offset = geometry::frame_offset(board.elements, shot.elements, pairs);
    let mut found = Vec::new();
    if offset.0.abs() > EDGE_TOLERANCE || offset.1.abs() > EDGE_TOLERANCE {
        found.push(Warning::FrameOffset {
            dx: offset.0,
            dy: offset.1,
        });
    }
    found.extend(geometry::boxes(
        board.elements,
        shot.elements,
        pairs,
        offset,
    ));
    found.extend(geometry::fonts(board.elements, shot.elements, pairs));
    found.extend(colour::pairs(board, shot, pairs));
    found.extend(
        regions::regions(board, shot, offset)
            .into_iter()
            .map(|r| Warning::Region {
                bounds: r.bounds,
                delta: r.delta,
            }),
    );
    found
}

fn differences(board: &Entry, shot: &Entry) -> Vec<Aspect> {
    let mut differing = Vec::new();
    if board.value != shot.value {
        differing.push(Aspect::Value);
    }
    // AppKit also reports inherited enablement on static text.
    if board.kind != Role::Text && board.enabled != shot.enabled {
        differing.push(Aspect::Enabled);
    }
    if board.selected != shot.selected {
        differing.push(Aspect::Selected);
    }
    differing
}

/// "the button “Set up”", or "the unlabelled button".
fn the(entry: &Entry) -> String {
    if entry.words.is_empty() {
        format!("the unlabelled {}", entry.kind.word())
    } else {
        format!("the {} “{}”", entry.kind.word(), entry.words)
    }
}

fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

fn quoted(value: &str) -> String {
    if value.is_empty() {
        "no value".to_string()
    } else {
        format!("“{value}”")
    }
}

impl Finding {
    /// The finding as a sentence for the report.
    pub(crate) fn sentence(&self) -> String {
        match self {
            Self::Missing(entry) => format!("Shot is missing {}", the(entry)),
            Self::Extra(entry) => {
                format!("Shot has {}, which the board does not", the(entry))
            }
            Self::OutOfOrder { board, .. } => {
                let mut sentence = the(board);
                sentence.replace_range(..1, "T");
                format!("{sentence} is in another place in the shot's reading order")
            }
            Self::RoleDiffers { board, shot } => format!(
                "“{}” is {} {} on the board and {} {} in the shot",
                board.words,
                article(board.kind.word()),
                board.kind.word(),
                article(shot.kind.word()),
                shot.kind.word()
            ),
            Self::StateDiffers {
                board,
                shot,
                aspect,
            } => {
                let mut subject = the(board);
                subject.replace_range(..1, "T");
                match aspect {
                    Aspect::Value => format!(
                        "{subject} has the value {} on the board and {} in the shot",
                        quoted(&aspect.of(board)),
                        quoted(&aspect.of(shot))
                    ),
                    Aspect::Enabled | Aspect::Selected => format!(
                        "{subject} is {} on the board and {} in the shot",
                        aspect.of(board),
                        aspect.of(shot)
                    ),
                }
            }
        }
    }
}

/// A length in points as a sentence writes it: `4`, `2.5`.
fn pt(length: f64) -> String {
    let text = format!("{:.1}", length.abs());
    format!("{} pt", text.strip_suffix(".0").unwrap_or(&text))
}

impl Warning {
    /// The board's element the warning is about; none for a warning about the
    /// whole frame or an area of the picture.
    pub(crate) fn entry(&self) -> Option<&Entry> {
        match self {
            Self::Moved { entry, .. }
            | Self::Resized { entry, .. }
            | Self::FontSize { entry, .. }
            | Self::FontWeight { entry, .. }
            | Self::Colour { entry, .. } => Some(entry),
            Self::FrameOffset { .. } | Self::Region { .. } => None,
        }
    }

    /// The warning as a sentence for the report.
    pub(crate) fn sentence(&self) -> String {
        match self {
            Self::FrameOffset { dx, dy } => format!(
                "The shot's frame is {} {} and {} {} of the board's",
                pt(*dx),
                if *dx >= 0.0 { "right" } else { "left" },
                pt(*dy),
                if *dy >= 0.0 { "below" } else { "above" },
            ),
            Self::Moved { entry, dx, dy } => {
                let mut moves = Vec::new();
                if *dx != 0.0 {
                    moves.push(format!(
                        "{} {}",
                        pt(*dx),
                        if *dx > 0.0 { "right" } else { "left" }
                    ));
                }
                if *dy != 0.0 {
                    moves.push(format!(
                        "{} {}",
                        pt(*dy),
                        if *dy > 0.0 { "down" } else { "up" }
                    ));
                }
                format!(
                    "{} moved {} in the shot",
                    quoted_words(entry),
                    moves.join(" and ")
                )
            }
            Self::Resized { entry, dw, dh } => {
                let mut sizes = Vec::new();
                if *dw != 0.0 {
                    sizes.push(format!(
                        "{} {}",
                        pt(*dw),
                        if *dw > 0.0 { "wider" } else { "narrower" }
                    ));
                }
                if *dh != 0.0 {
                    sizes.push(format!(
                        "{} {}",
                        pt(*dh),
                        if *dh > 0.0 { "taller" } else { "shorter" }
                    ));
                }
                format!(
                    "{} is {} in the shot",
                    quoted_words(entry),
                    sizes.join(" and ")
                )
            }
            Self::FontSize { entry, board, shot } => format!(
                "{} is set at {} in the shot and {} on the board",
                quoted_words(entry),
                pt(*shot),
                pt(*board)
            ),
            Self::FontWeight { entry, board, shot } => format!(
                "{} is set at weight {shot} in the shot and {board} on the board",
                quoted_words(entry)
            ),
            Self::Colour {
                entry,
                part,
                board,
                shot,
                ..
            } => {
                let what = match part {
                    Part::Background => "background",
                    Part::Foreground => "foreground",
                    Part::Text => "colour",
                };
                format!(
                    "The {what} of {} is {} on the board and {} in the shot",
                    the(entry),
                    hex(*board),
                    hex(*shot)
                )
            }
            Self::Region { bounds, .. } => format!(
                "The pictures differ in the area {} wide and {} high at ({}, {})",
                pt(bounds.width),
                pt(bounds.height),
                bounds.x,
                bounds.y
            ),
        }
    }
}

/// "“Set up”", the words of an entry that has some.
fn quoted_words(entry: &Entry) -> String {
    if entry.words.is_empty() {
        format!("The unlabelled {}", entry.kind.word())
    } else {
        format!("“{}”", entry.words)
    }
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
