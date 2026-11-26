//! Regions: what neither element list holds (icons, dividers) and the image
//! elements, compared as pictures. A grid over the aligned pictures outside
//! the text and control boxes; a cell whose mean difference passes the
//! tolerance is marked, and neighbouring marked cells merge into rectangles.

use super::colour::{Lab, Rgb, delta_e, pixel};
use super::components::components;
use super::window::Window;
use super::{DELTA_E_TOLERANCE, PictureSide, REGION_CELL, Side};
use crate::shots::{Bounds, Element, Picture};

/// A marked region is alike when no pixel of it is more than this many
/// levels (a channel's level is 0 to 255) outside the range of the other
/// picture's pixels within one pixel of it, and those distances average
/// [`RESIDUAL_LEVELS`] or fewer: ink in the same places on both sides, up to
/// one pixel apart, differing by the antialiasing of its edges or a colour
/// rounded by a level, which neither the board nor the app can change. Ink
/// two pixels apart or more is not alike.
pub(super) const EDGE_LEVELS: u8 = 48;
/// The mean difference, in levels, of a region that is alike.
pub(super) const RESIDUAL_LEVELS: f64 = 2.0;

/// A marked area of the board's frame, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Region {
    pub(crate) bounds: Bounds,
    /// The greatest mean difference of a cell in it.
    pub(crate) delta: f64,
}

/// Whether `bounds` meets the cell at (`x`, `y`).
fn meets(bounds: Bounds, x: f64, y: f64) -> bool {
    bounds.x < x + REGION_CELL
        && bounds.x + bounds.width > x
        && bounds.y < y + REGION_CELL
        && bounds.y + bounds.height > y
}

/// The boxes of the elements whose pictures other checks cover, in the
/// frame of the list's own picture.
fn covered(list: &[Element], offset: (f64, f64)) -> Vec<Bounds> {
    list.iter()
        .filter(|e| e.role == crate::shots::Role::Text || e.role.is_control())
        .map(|e| Bounds {
            x: e.bounds.x - offset.0,
            y: e.bounds.y - offset.1,
            ..e.bounds
        })
        .collect()
}

/// The pixels of the two pictures at the point (`px`, `py`) of the board's
/// frame, as (column, row) of each picture: none where the window leaves the
/// point out. The shot's frame is `offset` from the board's.
fn pixels_at(
    board: &Side,
    shot: &Side,
    offset: (f64, f64),
    window: Option<&Window>,
    (px, py): (f64, f64),
) -> Option<[(i64, i64); 2]> {
    if window.is_some_and(|window| {
        !window.compares(PictureSide::Board, board, px, py)
            || !window.compares(PictureSide::Shot, shot, px + offset.0, py + offset.1)
    }) {
        return None;
    }
    let at = |scale: f64, shift: (f64, f64)| {
        (
            ((px + shift.0) * scale).floor() as i64,
            ((py + shift.1) * scale).floor() as i64,
        )
    };
    Some([at(board.scale, (0.0, 0.0)), at(shot.scale, offset)])
}

/// The colours of the two pictures at the point (`px`, `py`) of the board's
/// frame, as the picture comparison reads them: none where either picture
/// has no pixel or the window leaves the point out.
fn colours_at(
    board: &Side,
    shot: &Side,
    offset: (f64, f64),
    window: Option<&Window>,
    point: (f64, f64),
) -> Option<(Rgb, Rgb)> {
    let [(bx, by), (sx, sy)] = pixels_at(board, shot, offset, window, point)?;
    Some((pixel(board.picture, bx, by)?, pixel(shot.picture, sx, sy)?))
}

/// How far each channel of `colour` lies outside the range of that channel
/// over the pixels within one pixel of (`x`, `y`) in `picture`, leaving out
/// positions past its edge.
fn outside_neighbourhood(picture: &Picture, (x, y): (i64, i64), colour: Rgb) -> [u8; 3] {
    let mut low = [u8::MAX; 3];
    let mut high = [u8::MIN; 3];
    for ny in y - 1..=y + 1 {
        for nx in x - 1..=x + 1 {
            if let Some(near) = pixel(picture, nx, ny) {
                for channel in 0..3 {
                    low[channel] = low[channel].min(near[channel]);
                    high[channel] = high[channel].max(near[channel]);
                }
            }
        }
    }
    std::array::from_fn(|channel| {
        colour[channel]
            .saturating_sub(high[channel])
            .max(low[channel].saturating_sub(colour[channel]))
    })
}

/// The mean difference of the two pictures over the cell at (`x`, `y`) of the
/// board's frame, sampled every point; the shot's frame is `offset` from the
/// board's.
fn cell_delta(
    board: &Side,
    shot: &Side,
    offset: (f64, f64),
    x: f64,
    y: f64,
    window: Option<&Window>,
) -> Option<f64> {
    let (mut total, mut count) = (0.0, 0);
    let steps = REGION_CELL as i64;
    for dy in 0..steps {
        for dx in 0..steps {
            let point = (x + dx as f64 + 0.5, y + dy as f64 + 0.5);
            if let Some((a, b)) = colours_at(board, shot, offset, window, point) {
                total += delta_e(Lab::of(a), Lab::of(b));
                count += 1;
            }
        }
    }
    (count > 0).then(|| total / f64::from(count))
}

/// Whether the pictures of the cells (columns and rows of the grid) differ
/// only by a residual ([`EDGE_LEVELS`], [`RESIDUAL_LEVELS`]): in each channel,
/// a pixel's difference is the larger of how far the board's value lies
/// outside the shot's range within one pixel of the pixel it maps to, and how
/// far the shot's value lies outside the board's range within one pixel of
/// the board's pixel. Compares every pixel of the board's picture in the
/// cells.
fn residual(
    board: &Side,
    shot: &Side,
    offset: (f64, f64),
    cells: &[(usize, usize)],
    window: Option<&Window>,
) -> bool {
    let (mut levels, mut count) = (0.0, 0_u32);
    let steps = (REGION_CELL * board.scale) as i64;
    for &(column, row) in cells {
        for dy in 0..steps {
            for dx in 0..steps {
                let x = (column as f64 * REGION_CELL * board.scale).floor() as i64 + dx;
                let y = (row as f64 * REGION_CELL * board.scale).floor() as i64 + dy;
                let point = (
                    (x as f64 + 0.5) / board.scale,
                    (y as f64 + 0.5) / board.scale,
                );
                let Some([board_pixel, shot_pixel]) = pixels_at(board, shot, offset, window, point)
                else {
                    continue;
                };
                let (Some(a), Some(b)) = (
                    pixel(board.picture, board_pixel.0, board_pixel.1),
                    pixel(shot.picture, shot_pixel.0, shot_pixel.1),
                ) else {
                    continue;
                };
                let board_outside = outside_neighbourhood(shot.picture, shot_pixel, a);
                let shot_outside = outside_neighbourhood(board.picture, board_pixel, b);
                let differences: [u8; 3] = std::array::from_fn(|channel| {
                    board_outside[channel].max(shot_outside[channel])
                });
                if differences.iter().any(|&d| d > EDGE_LEVELS) {
                    return false;
                }
                levels += differences.iter().map(|&d| f64::from(d)).sum::<f64>() / 3.0;
                count += 1;
            }
        }
    }
    count > 0 && levels / f64::from(count) <= RESIDUAL_LEVELS
}

/// The regions where the pictures differ outside the covered boxes.
pub(super) fn regions(
    board: &Side,
    shot: &Side,
    offset: (f64, f64),
    window: Option<&Window>,
) -> Vec<Region> {
    let covers: Vec<Bounds> = covered(board.elements, (0.0, 0.0))
        .into_iter()
        .chain(covered(shot.elements, offset))
        .chain(board.unshot.iter().copied())
        .chain(shot.unshot.iter().map(|b| Bounds {
            x: b.x - offset.0,
            y: b.y - offset.1,
            ..*b
        }))
        .collect();
    let width = f64::from(board.picture.width) / board.scale;
    let height = f64::from(board.picture.height) / board.scale;
    let columns = (width / REGION_CELL).ceil() as usize;
    let rows = (height / REGION_CELL).ceil() as usize;
    let mut marked: Vec<Vec<Option<f64>>> = vec![vec![None; columns]; rows];
    for (row, cells) in marked.iter_mut().enumerate() {
        for (column, cell) in cells.iter_mut().enumerate() {
            let (x, y) = (column as f64 * REGION_CELL, row as f64 * REGION_CELL);
            if covers.iter().any(|b| meets(*b, x, y)) {
                continue;
            }
            *cell =
                cell_delta(board, shot, offset, x, y, window).filter(|d| *d > DELTA_E_TOLERANCE);
        }
    }
    // Neighbouring marked cells (sharing an edge) are one region.
    components(columns, rows, |column, row| marked[row][column].is_some())
        .into_iter()
        .filter(|component| !residual(board, shot, offset, &component.cells, window))
        .map(|component| {
            let (left, top, right, bottom) = component.extent();
            let delta = component
                .cells
                .iter()
                .filter_map(|&(column, row)| marked[row][column])
                .fold(0.0, f64::max);
            Region {
                bounds: Bounds {
                    x: left as f64 * REGION_CELL,
                    y: top as f64 * REGION_CELL,
                    width: (right - left + 1) as f64 * REGION_CELL,
                    height: (bottom - top + 1) as f64 * REGION_CELL,
                },
                delta,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "regions_tests.rs"]
mod tests;
