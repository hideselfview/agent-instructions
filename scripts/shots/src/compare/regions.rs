//! Regions: what neither element list holds (icons, dividers) and the image
//! elements, compared as pictures. A grid over the aligned pictures outside
//! the text and control boxes; a cell whose mean difference passes the
//! tolerance is marked, and neighbouring marked cells merge into rectangles.

use super::colour::{Lab, delta_e, pixel};
use super::corners::Corners;
use super::{DELTA_E_TOLERANCE, PictureSide, REGION_CELL, Side};
use crate::shots::{Bounds, Element, Picture};

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

/// The mean difference of the two pictures over the cell at (`x`, `y`) of the
/// board's frame, sampled every point; the shot's frame is `offset` from the
/// board's.
fn cell_delta(
    board: &Side,
    shot: &Side,
    offset: (f64, f64),
    x: f64,
    y: f64,
    corners: Option<&Corners>,
) -> Option<f64> {
    let (mut total, mut count) = (0.0, 0);
    let steps = REGION_CELL as i64;
    for dy in 0..steps {
        for dx in 0..steps {
            let (px, py) = (x + dx as f64 + 0.5, y + dy as f64 + 0.5);
            if corners.is_some_and(|corners| {
                !corners.contains(PictureSide::Board, board, px, py)
                    || !corners.contains(PictureSide::Shot, shot, px + offset.0, py + offset.1)
            }) {
                continue;
            }
            let at = |picture: &Picture, scale: f64, shift: (f64, f64)| {
                pixel(
                    picture,
                    ((px + shift.0) * scale).floor() as i64,
                    ((py + shift.1) * scale).floor() as i64,
                )
            };
            if let (Some(a), Some(b)) = (
                at(board.picture, board.scale, (0.0, 0.0)),
                at(shot.picture, shot.scale, offset),
            ) {
                total += delta_e(Lab::of(a), Lab::of(b));
                count += 1;
            }
        }
    }
    (count > 0).then(|| total / f64::from(count))
}

/// The regions where the pictures differ outside the covered boxes.
pub(super) fn regions(
    board: &Side,
    shot: &Side,
    offset: (f64, f64),
    corners: Option<&Corners>,
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
                cell_delta(board, shot, offset, x, y, corners).filter(|d| *d > DELTA_E_TOLERANCE);
        }
    }
    // Neighbouring marked cells (sharing an edge) are one region.
    let mut seen = vec![vec![false; columns]; rows];
    let mut found = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            if seen[row][column] || marked[row][column].is_none() {
                continue;
            }
            let mut stack = vec![(row, column)];
            seen[row][column] = true;
            let (mut top, mut left, mut bottom, mut right) = (row, column, row, column);
            let mut delta: f64 = 0.0;
            while let Some((r, c)) = stack.pop() {
                top = top.min(r);
                bottom = bottom.max(r);
                left = left.min(c);
                right = right.max(c);
                delta = delta.max(marked[r][c].unwrap_or(0.0));
                let neighbours = [
                    (r.wrapping_sub(1), c),
                    (r + 1, c),
                    (r, c.wrapping_sub(1)),
                    (r, c + 1),
                ];
                for (nr, nc) in neighbours {
                    if nr < rows && nc < columns && !seen[nr][nc] && marked[nr][nc].is_some() {
                        seen[nr][nc] = true;
                        stack.push((nr, nc));
                    }
                }
            }
            found.push(Region {
                bounds: Bounds {
                    x: left as f64 * REGION_CELL,
                    y: top as f64 * REGION_CELL,
                    width: (right - left + 1) as f64 * REGION_CELL,
                    height: (bottom - top + 1) as f64 * REGION_CELL,
                },
                delta,
            });
        }
    }
    found
}

#[cfg(test)]
#[path = "regions_tests.rs"]
mod tests;
