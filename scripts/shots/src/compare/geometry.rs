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
/// offset of the matched pairs' top-left corners.
pub(crate) fn frame_offset(
    board: &[Element],
    shot: &[Element],
    pairs: &[(Entry, Entry)],
) -> (f64, f64) {
    // A symbol's corner is wherever the platform puts its glyph in the box,
    // so a symbol says nothing of the frame.
    let (dx, dy): (Vec<f64>, Vec<f64>) = pairs
        .iter()
        .filter_map(|(b, s)| {
            let (from, to) = (b.bounds(board), shot[s.index].bounds);
            (!is_glyph_in_box(b.kind, &from, &to)).then_some((to.x - from.x, to.y - from.y))
        })
        .unzip();
    (median(dx), median(dy))
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

/// The geometry warnings of the pairs, the shot's boxes moved back by `offset`.
pub(crate) fn boxes(
    board: &[Element],
    shot: &[Element],
    pairs: &[(Entry, Entry)],
    offset: (f64, f64),
) -> Vec<Warning> {
    let mut warnings = Vec::new();
    for (b, s) in pairs {
        let (from, to) = (b.bounds(board), &shot[s.index]);
        let to_bounds = Bounds {
            x: to.bounds.x - offset.0,
            y: to.bounds.y - offset.1,
            ..to.bounds
        };
        let (horizontal, vertical) = if b.kind == Role::Text {
            // A platform's text box may span its container, so its left edge
            // and width say something only where the two widths agree.
            let widths_agree = (to_bounds.width - from.width).abs() <= EDGE_TOLERANCE;
            (
                widths_agree
                    .then(|| {
                        Axis::start_and_length(from.x, from.width, to_bounds.x, to_bounds.width)
                    })
                    .flatten(),
                Axis::start_and_length(from.y, from.height, to_bounds.y, to_bounds.height),
            )
        } else if is_glyph_in_box(b.kind, &from, &to_bounds) && lies_inside(&from, &to_bounds) {
            (None, None)
        } else {
            (
                Axis::edges(from.x, from.width, to_bounds.x, to_bounds.width),
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
