//! Geometry: where the pairs sit in their frames, after the frames are
//! aligned, and how their text is set.

use super::normalize::Entry;
use super::{EDGE_TOLERANCE, FONT_SIZE_TOLERANCE, Warning};
use crate::shots::{Bounds, Element, Role};

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    }
}

/// How far the shot's frame sits right and down of the board's: the median
/// offset of the matched pairs' top-left corners. Horizontally the median
/// takes the pairs whose left edge is where the element is drawn, and only
/// when there are none, the texts of another width.
pub(crate) fn frame_offset(
    board: &[Element],
    shot: &[Element],
    pairs: &[(Entry, Entry)],
) -> (f64, f64) {
    let (mut dx, mut dy, mut other_width) = (Vec::new(), Vec::new(), Vec::new());
    for (b, s) in pairs {
        let (from, to) = (b.bounds(board), shot[s.index].bounds);
        // A symbol's corner is wherever the platform puts its glyph in the
        // box, so a symbol says nothing of the frame.
        if is_glyph_in_box(b.kind, &from, &to) {
            continue;
        }
        dy.push(to.y - from.y);
        // A text box of another width is another line layout (the board
        // wraps the line, the shot spans its container) and the text sits
        // in it by its alignment, so its left edge is a worse witness of the
        // frame than the left edge of a box that is drawn where it is.
        if b.kind != Role::Text || widths_agree(&from, &to, 0.0) {
            dx.push(to.x - from.x);
        } else {
            other_width.push(to.x - from.x);
        }
    }
    if dx.is_empty() {
        dx = other_width;
    }
    (median(dx), median(dy))
}

/// Whether two text boxes have the same width within the tolerance and
/// <slack> points more.
fn widths_agree(board: &Bounds, shot: &Bounds, slack: f64) -> bool {
    (shot.width - board.width).abs() <= EDGE_TOLERANCE + slack
}

/// How one axis of a box differs: the shift of its start, and how much longer
/// it is.
struct Axis {
    shift: f64,
    grow: f64,
}

impl Axis {
    /// All of the box: each edge may move by the tolerance.
    fn edges(board_start: f64, board_len: f64, shot_start: f64, shot_len: f64) -> Option<Self> {
        let start = shot_start - board_start;
        let end = (shot_start + shot_len) - (board_start + board_len);
        if start.abs() <= EDGE_TOLERANCE && end.abs() <= EDGE_TOLERANCE {
            return None;
        }
        let grow = end - start;
        // The part of the shift both edges share; a box that grew on one side
        // or about its middle has none.
        let common = if start * end > 0.0 {
            start.signum() * start.abs().min(end.abs())
        } else {
            0.0
        };
        // An edge past the tolerance that neither a size change nor a shared
        // shift explains is read as a shift.
        let shift = if grow.abs() <= EDGE_TOLERANCE && common.abs() <= EDGE_TOLERANCE {
            if start.abs() >= end.abs() { start } else { end }
        } else {
            common
        };
        Some(Self { shift, grow })
    }

    /// This axis with a shift of at most <place> and a growth of at most
    /// <width> left out.
    fn within(self, width: f64, place: f64) -> Self {
        Self {
            shift: if self.shift.abs() <= EDGE_TOLERANCE + place {
                0.0
            } else {
                self.shift
            },
            grow: if self.grow.abs() <= EDGE_TOLERANCE + width {
                0.0
            } else {
                self.grow
            },
        }
    }

    /// A text's start and its length.
    fn start_and_length(
        board_start: f64,
        board_len: f64,
        shot_start: f64,
        shot_len: f64,
    ) -> Option<Self> {
        let shift = shot_start - board_start;
        let grow = shot_len - board_len;
        (shift.abs() > EDGE_TOLERANCE || grow.abs() > EDGE_TOLERANCE)
            .then_some(Self { shift, grow })
    }
}

/// Whether a pair of images is a symbol the Mac boxes by the glyph it draws:
/// no larger than the board's box, which is as high as the line the symbol
/// sits in. The glyph's place in that box is the platform's, so the sides
/// share only that the glyph lies inside the box.
fn is_glyph_in_box(kind: Role, board: &Bounds, shot: &Bounds) -> bool {
    kind == Role::Image
        && shot.width <= board.width + EDGE_TOLERANCE
        && shot.height <= board.height + EDGE_TOLERANCE
}

/// Whether `shot` lies inside `board`, each edge within the tolerance.
fn lies_inside(board: &Bounds, shot: &Bounds) -> bool {
    shot.x >= board.x - EDGE_TOLERANCE
        && shot.y >= board.y - EDGE_TOLERANCE
        && shot.x + shot.width <= board.x + board.width + EDGE_TOLERANCE
        && shot.y + shot.height <= board.y + board.height + EDGE_TOLERANCE
}

/// The glyphs of the labels of <pairs> that start at or left of the left edge
/// of <at>, a box of <board>, and share most of its height: its own label and
/// those before it on its line.
fn glyphs_up_to(board: &[Element], pairs: &[(Entry, Entry)], at: &Bounds) -> usize {
    pairs
        .iter()
        .map(|(other, _)| other)
        .filter(|other| {
            let there = other.bounds(board);
            let shared = (at.y + at.height).min(there.y + there.height) - at.y.max(there.y);
            there.x <= at.x && shared > at.height.min(there.height) / 2.0
        })
        .map(|labelled| labelled.words.chars().count())
        .sum()
}

/// Whether `shot` is a container of the board's text box: wider, and holding
/// it on every side, each edge within the tolerance, with its middle height
/// the board's within the tolerance.
fn spans_container(board: &Bounds, shot: &Bounds) -> bool {
    let middle = |b: &Bounds| b.y + b.height / 2.0;
    lies_inside(shot, board)
        && shot.width > board.width + EDGE_TOLERANCE
        && (middle(shot) - middle(board)).abs() <= EDGE_TOLERANCE
}

/// The geometry warnings of the pairs, the shot's boxes moved back by `offset`.
/// A platform that rounds each glyph advance to a whole pixel (Android's
/// native text) sets a text's width and place off by `glyph_slack` points for
/// each glyph that sets them: a width by the glyphs of its label, a place by
/// the glyphs of the labels before it on its line and its own. It holds for
/// every element with a label, a text and a control alike (a chip's width and
/// place come from its label); an element no label sizes differs by less than
/// the box-edge tolerance either way.
pub(crate) fn boxes(
    board: &[Element],
    shot: &[Element],
    pairs: &[(Entry, Entry)],
    offset: (f64, f64),
    glyph_slack: f64,
) -> Vec<Warning> {
    let mut warnings = Vec::new();
    for (b, s) in pairs {
        let (from, to) = (b.bounds(board), &shot[s.index]);
        let to_bounds = Bounds {
            x: to.bounds.x - offset.0,
            y: to.bounds.y - offset.1,
            ..to.bounds
        };
        let width_slack = glyph_slack * b.words.chars().count() as f64;
        let place_slack = glyph_slack * glyphs_up_to(board, pairs, &from) as f64;
        let (horizontal, vertical) = if b.kind == Role::Text {
            // A platform's text box may span its container, so its left edge
            // and width say something only where the two widths agree.
            (
                widths_agree(&from, &to_bounds, width_slack)
                    .then(|| {
                        Axis::start_and_length(from.x, from.width, to_bounds.x, to_bounds.width)
                    })
                    .flatten()
                    .map(|axis| axis.within(width_slack, place_slack)),
                // A text box that spans its container, as the Mac's window
                // title spans its bar, holds the board's box and shares its
                // middle height: the text sits where the board draws it.
                (!spans_container(&from, &to_bounds))
                    .then(|| {
                        Axis::start_and_length(from.y, from.height, to_bounds.y, to_bounds.height)
                    })
                    .flatten(),
            )
        } else if is_glyph_in_box(b.kind, &from, &to_bounds) && lies_inside(&from, &to_bounds) {
            (None, None)
        } else {
            (
                Axis::edges(from.x, from.width, to_bounds.x, to_bounds.width)
                    .map(|axis| axis.within(width_slack, place_slack)),
                Axis::edges(from.y, from.height, to_bounds.y, to_bounds.height),
            )
        };
        let moved = |axis: &Option<Axis>| {
            axis.as_ref().map_or(0.0, |a| {
                if a.shift.abs() > EDGE_TOLERANCE {
                    a.shift
                } else {
                    0.0
                }
            })
        };
        let grown = |axis: &Option<Axis>| {
            axis.as_ref().map_or(0.0, |a| {
                if a.grow.abs() > EDGE_TOLERANCE {
                    a.grow
                } else {
                    0.0
                }
            })
        };
        let (dx, dy) = (moved(&horizontal), moved(&vertical));
        if dx != 0.0 || dy != 0.0 {
            warnings.push(Warning::Moved {
                entry: b.clone(),
                dx,
                dy,
            });
        }
        let (dw, dh) = (grown(&horizontal), grown(&vertical));
        if dw != 0.0 || dh != 0.0 {
            warnings.push(Warning::Resized {
                entry: b.clone(),
                dw,
                dh,
            });
        }
    }
    warnings
}

/// The font warnings of the text pairs: size past the tolerance, and any
/// weight difference, where both sides give them.
pub(crate) fn fonts(board: &[Element], shot: &[Element], pairs: &[(Entry, Entry)]) -> Vec<Warning> {
    let mut warnings = Vec::new();
    for (b, s) in pairs {
        let (Some(from), Some(to)) = (&board[b.index].text, &shot[s.index].text) else {
            continue;
        };
        if let (Some(from), Some(to)) = (from.size, to.size)
            && (from - to).abs() > FONT_SIZE_TOLERANCE
        {
            warnings.push(Warning::FontSize {
                entry: b.clone(),
                board: from,
                shot: to,
            });
        }
        if let (Some(from), Some(to)) = (from.weight, to.weight)
            && from != to
        {
            warnings.push(Warning::FontWeight {
                entry: b.clone(),
                board: from,
                shot: to,
            });
        }
    }
    warnings
}

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;
