//! The outer cutouts of a macOS window, in each picture's own frame.

use std::cmp::Ordering;

use super::{PictureSide, Side, rgba};

/// The corner search reaches past macOS's 23.5 pt curve.
const CORNER_POINTS: f64 = 32.0;
const EDGE_TOLERANCE_PIXELS: usize = 1;
const HALF_ALPHA: u8 = 128;
/// Chrome composites opaque boards onto the page. Its corner boundary can
/// appear as a change of fill or as the highlight just inside the window.
const CORNER_CONTRAST: u8 = 5;

pub(super) struct Corners {
    scale: f64,
    limit: usize,
    cutouts: [Option<Cutout>; 4],
}

struct Cutout {
    /// Insets along diagonals that run inward from the two frame edges.
    /// Index `limit - 1` starts at the exact corner.
    insets: Vec<Insets>,
    differs: bool,
}

struct Insets {
    board: usize,
    shot: usize,
}

impl Corners {
    /// The shot's alpha identifies a native rounded window. Mac component
    /// shots drawn into a rectangular image have opaque corners.
    pub(super) fn of(board: &Side, shot: &Side) -> Option<Self> {
        let scale = board.scale.min(shot.scale);
        let limit = [board, shot]
            .into_iter()
            .map(|side| f64::from(side.picture.width.min(side.picture.height)) / (2.0 * side.scale))
            .fold(CORNER_POINTS, f64::min);
        let limit = (limit * scale).floor() as usize;
        if limit == 0 {
            return None;
        }
        let samples = |side| {
            std::array::from_fn::<_, 4, _>(|corner| Samples {
                side,
                corner,
                scale,
            })
        };
        let (board, shot) = (samples(board), samples(shot));
        let board_has_alpha = board.iter().any(|samples| samples.at(0, 0)[3] < HALF_ALPHA);
        let cutouts = std::array::from_fn(|corner| {
            let shot = &shot[corner];
            (shot.at(0, 0)[3] < HALF_ALPHA && shot.at(limit - 1, limit - 1)[3] == 255)
                .then(|| Cutout::read(&board[corner], shot, board_has_alpha, limit))
        });
        cutouts.iter().any(Option::is_some).then_some(Self {
            scale,
            limit,
            cutouts,
        })
    }

    pub(super) fn differ(&self) -> bool {
        self.cutouts.iter().flatten().any(|cutout| cutout.differs)
    }

    /// Whether a sample in the side's own frame lands inside the window.
    pub(super) fn contains(&self, picture: PictureSide, side: &Side, x: f64, y: f64) -> bool {
        let sample = |point: f64, length: u32| {
            let length = f64::from(length);
            let pixel = (point * side.scale).floor() + 0.5;
            let far = pixel >= length / 2.0;
            let distance = if far { length - pixel } else { pixel };
            ((distance * self.scale / side.scale).floor() as usize, far)
        };
        let (x, right) = sample(x, side.picture.width);
        let (y, bottom) = sample(y, side.picture.height);
        if x >= self.limit || y >= self.limit {
            return true;
        }
        let corner = usize::from(right) + 2 * usize::from(bottom);
        let ray = self.limit - 1 + x - y;
        self.cutouts[corner].as_ref().is_none_or(|cutout| {
            let insets = &cutout.insets[ray];
            let inset = match picture {
                PictureSide::Board => insets.board,
                PictureSide::Shot => insets.shot,
            };
            x.min(y) >= inset
        })
    }
}

/// A corner viewed on the coarser picture's pixel grid, with the origin
/// reflected to the tip of the corner.
struct Samples<'a, 'picture> {
    side: &'a Side<'picture>,
    corner: usize,
    scale: f64,
}

impl Samples<'_, '_> {
    fn at(&self, x: usize, y: usize) -> [u8; 4] {
        let picture = self.side.picture;
        let index = |distance| ((distance as f64 + 0.5) * self.side.scale / self.scale) as i64;
        let x = index(x);
        let y = index(y);
        let x = if self.corner & 1 != 0 {
            i64::from(picture.width) - 1 - x
        } else {
            x
        };
        let y = if self.corner >= 2 {
            i64::from(picture.height) - 1 - y
        } else {
            y
        };
        rgba(picture, x, y).expect("corner samples fit in the picture")
    }
}

impl Cutout {
    fn read(board: &Samples, shot: &Samples, board_has_alpha: bool, limit: usize) -> Self {
        let backdrop = board.at(0, 0);
        let mut rays = Vec::with_capacity(2 * limit - 1);
        for ray in 0..2 * limit - 1 {
            let x = ray.saturating_sub(limit - 1);
            let y = (limit - 1).saturating_sub(ray);
            let length = limit - x.max(y);
            let shot_edge = (0..length)
                .find(|d| shot.at(x + d, y + d)[3] >= HALF_ALPHA)
                .unwrap_or(length);
            let shot_inset = (shot_edge..length)
                .find(|d| shot.at(x + d, y + d)[3] == 255)
                .unwrap_or(length);
            let board_edge = (0..length).find(|d| {
                let pixel = board.at(x + d, y + d);
                if board_has_alpha {
                    pixel[3] >= HALF_ALPHA
                } else {
                    pixel[..3]
                        .iter()
                        .zip(backdrop)
                        .any(|(&a, b)| a.abs_diff(b) >= CORNER_CONTRAST)
                }
            });
            let board_edge = if board_has_alpha {
                Some(board_edge.unwrap_or(length))
            } else {
                // A ray whose first pixel already differs from the backdrop
                // gives no evidence of where an opaque board's outline is.
                board_edge.filter(|edge| *edge > 0 && shot_edge < length)
            };
            let difference = board_edge.map_or(Ordering::Equal, |edge| {
                if edge.abs_diff(shot_edge) > EDGE_TOLERANCE_PIXELS {
                    edge.cmp(&shot_edge)
                } else {
                    Ordering::Equal
                }
            });
            let board_inset = if board_has_alpha {
                (board_edge.expect("alpha gives an extent even for an empty ray")..length)
                    .find(|d| board.at(x + d, y + d)[3] == 255)
                    .unwrap_or(length)
            } else {
                // The flattened board's first visible edge pixel may still
                // blend with the page. Keep one pixel clear of that boundary.
                board_edge.map_or(shot_inset, |edge| shot_inset.max(edge + 1))
            };
            rays.push((
                Insets {
                    board: board_inset,
                    shot: shot_inset,
                },
                difference,
            ));
        }
        let mut differs = false;
        // A board flattened onto its backdrop may have no visible edge. Its
        // native counterpart supplies the mask until adjacent rays show a
        // consistent shift. A single colour change can be a shadow or control.
        for group in rays.chunk_by_mut(|a, b| a.1 == b.1) {
            let edge_changed = group[0].1 != Ordering::Equal;
            let shifted = edge_changed && (board_has_alpha || group.len() >= 3);
            differs |= shifted;
            if !board_has_alpha {
                for (insets, _) in group {
                    // The board and the native outline can agree within one
                    // pixel even when Chrome blends a pixel farther inward.
                    let interior = insets.shot + usize::from(insets.shot > 0);
                    insets.board = if edge_changed && !shifted {
                        interior
                    } else {
                        insets.board.max(interior)
                    };
                }
            }
        }
        let insets = rays.into_iter().map(|(insets, _)| insets).collect();
        Self { insets, differs }
    }
}
