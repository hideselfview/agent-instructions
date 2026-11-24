//! The close, minimise and zoom buttons of a macOS window, found in each
//! picture. The shot's element list leaves the system's buttons out and a
//! board draws its own, so only the pictures show where each side has them.

use std::collections::HashMap;

use super::corners::Corners;
use super::{EDGE_TOLERANCE, PictureSide, Side, rgba};
use crate::shots::Bounds;

/// The window's top left, in points, where the buttons sit.
const STRIP: (f64, f64) = (96.0, 48.0);
/// A button is this many points across, in each direction.
const SIZE: std::ops::RangeInclusive<f64> = 10.0..=18.0;
/// A pixel belongs to a button when a channel differs this much from the
/// strip's backdrop.
const CONTRAST: u8 = 12;
/// A round button fills at least this much of its box.
const FILL: f64 = 0.6;
/// The points around a button that the picture comparison leaves out too, for
/// the anti-aliased edge.
const MARGIN: f64 = 1.0;

/// Each side's buttons, as boxes in the side's own frame, from left to right.
pub(super) struct Buttons {
    board: Vec<Bounds>,
    shot: Vec<Bounds>,
}

impl Buttons {
    pub(super) fn of(board: &Side, shot: &Side, corners: &Corners) -> Self {
        Self {
            board: find(PictureSide::Board, board, corners),
            shot: find(PictureSide::Shot, shot, corners),
        }
    }

    /// Whether the sides have another count of buttons, or one in another
    /// place; the shot's frame is `offset` from the board's.
    pub(super) fn differ(&self, offset: (f64, f64)) -> bool {
        let middle = |b: &Bounds| (b.x + b.width / 2.0, b.y + b.height / 2.0);
        self.board.len() != self.shot.len()
            || self.board.iter().zip(&self.shot).any(|(board, shot)| {
                let (board, shot) = (middle(board), middle(shot));
                (board.0 - (shot.0 - offset.0)).abs() > EDGE_TOLERANCE
                    || (board.1 - (shot.1 - offset.1)).abs() > EDGE_TOLERANCE
            })
    }

    /// Whether a sample in the picture's own frame lands on a button.
    pub(super) fn contain(&self, picture: PictureSide, x: f64, y: f64) -> bool {
        let boxes = match picture {
            PictureSide::Board => &self.board,
            PictureSide::Shot => &self.shot,
        };
        boxes.iter().any(|b| {
            x >= b.x - MARGIN
                && x < b.x + b.width + MARGIN
                && y >= b.y - MARGIN
                && y < b.y + b.height + MARGIN
        })
    }
}

/// The round marks in the top left of the window that a side draws over its
/// backdrop, the colour most of the strip's window pixels have.
fn find(picture: PictureSide, side: &Side, corners: &Corners) -> Vec<Bounds> {
    let scale = side.scale;
    let wide = ((STRIP.0 * scale) as usize).min(side.picture.width as usize);
    let high = ((STRIP.1 * scale) as usize).min(side.picture.height as usize);
    let inside = |x: usize, y: usize| {
        corners.contains(
            picture,
            side,
            (x as f64 + 0.5) / scale,
            (y as f64 + 0.5) / scale,
        )
    };
    let colour = |x: usize, y: usize| {
        let [r, g, b, _] =
            rgba(side.picture, x as i64, y as i64).expect("the strip is in the picture");
        [r, g, b]
    };
    let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
    for y in 0..high {
        for x in 0..wide {
            if inside(x, y) {
                *counts.entry(colour(x, y).map(|c| c >> 3)).or_default() += 1;
            }
        }
    }
    let Some((backdrop, _)) = counts.into_iter().max_by_key(|(key, count)| (*count, *key)) else {
        return Vec::new();
    };
    let backdrop = backdrop.map(|c| (c << 3) + 4);
    let marked = |x: usize, y: usize| {
        inside(x, y)
            && colour(x, y)
                .iter()
                .zip(backdrop)
                .any(|(&c, b)| c.abs_diff(b) >= CONTRAST)
    };
    let mut seen = vec![false; wide * high];
    let mut found = Vec::new();
    for start in 0..wide * high {
        if seen[start] || !marked(start % wide, start / wide) {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let (mut left, mut top, mut right, mut bottom) = (wide, high, 0, 0);
        let mut count = 0;
        while let Some(at) = stack.pop() {
            let (x, y) = (at % wide, at / wide);
            count += 1;
            (left, right) = (left.min(x), right.max(x));
            (top, bottom) = (top.min(y), bottom.max(y));
            let around = [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ];
            for (nx, ny) in around {
                if nx < wide && ny < high && !seen[ny * wide + nx] && marked(nx, ny) {
                    seen[ny * wide + nx] = true;
                    stack.push(ny * wide + nx);
                }
            }
        }
        let bounds = Bounds {
            x: left as f64 / scale,
            y: top as f64 / scale,
            width: (right - left + 1) as f64 / scale,
            height: (bottom - top + 1) as f64 / scale,
        };
        let area = (right - left + 1) * (bottom - top + 1);
        if SIZE.contains(&bounds.width)
            && SIZE.contains(&bounds.height)
            && count as f64 >= FILL * area as f64
        {
            found.push(bounds);
        }
    }
    found.sort_by(|a, b| a.x.total_cmp(&b.x));
    found
}
