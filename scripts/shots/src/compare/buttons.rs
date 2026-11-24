//! The close, minimise and zoom buttons of a macOS window, found in each
//! picture. The shot's element list leaves the system's buttons out and a
//! board draws its own, so only the pictures show where each side has them.

use std::collections::HashMap;

use super::colour::{FAR_FROM_BACKGROUND, Rgb, background_of, pixel, rgb_delta};
use super::components::components;
use super::corners::Corners;
use super::{EDGE_TOLERANCE, PictureSide, Side};
use crate::shots::Bounds;

/// The window's top left, in points, where the buttons sit.
const STRIP: (f64, f64) = (96.0, 48.0);
/// A button is this many points across, in each direction.
const SIZE: std::ops::RangeInclusive<f64> = 10.0..=18.0;
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
        pixel(side.picture, x as i64, y as i64).expect("the strip is in the picture")
    };
    let mut counts: HashMap<Rgb, usize> = HashMap::new();
    for y in 0..high {
        for x in 0..wide {
            if inside(x, y) {
                *counts.entry(colour(x, y)).or_default() += 1;
            }
        }
    }
    let Some((backdrop, _)) = background_of(&counts) else {
        return Vec::new();
    };
    let marked = |x: usize, y: usize| {
        inside(x, y) && rgb_delta(backdrop, colour(x, y)) > FAR_FROM_BACKGROUND
    };
    let mut found = Vec::new();
    for component in components(wide, high, marked) {
        let (left, top, right, bottom) = component.extent();
        let bounds = Bounds {
            x: left as f64 / scale,
            y: top as f64 / scale,
            width: (right - left + 1) as f64 / scale,
            height: (bottom - top + 1) as f64 / scale,
        };
        let area = (right - left + 1) * (bottom - top + 1);
        if SIZE.contains(&bounds.width)
            && SIZE.contains(&bounds.height)
            && component.cells.len() as f64 >= FILL * area as f64
        {
            found.push(bounds);
        }
    }
    found.sort_by(|a, b| a.x.total_cmp(&b.x));
    found
}
