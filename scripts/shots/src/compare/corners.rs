//! The outer cutouts of a macOS window, in each picture's own frame.

use std::cmp::Ordering;

use super::{PictureSide, Side, rgba};
use crate::shots::WindowShape;

/// The corner search reaches past macOS's 23.5 pt curve.
const CORNER_POINTS: f64 = 32.0;
const EDGE_TOLERANCE_PIXELS: usize = 1;
const HALF_ALPHA: u8 = 128;

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
    /// shots drawn into a rectangular image have opaque corners. The board's
    /// corners are those of `window`, the window its page draws; a board with
    /// no window read has none to compare.
    pub(super) fn of(board: &Side, shot: &Side, window: Option<&WindowShape>) -> Option<Self> {
        let window = window?;
        let scale = board.scale.min(shot.scale);
        let limit = [board, shot]
            .into_iter()
            .map(|side| f64::from(side.picture.width.min(side.picture.height)) / (2.0 * side.scale))
            .fold(CORNER_POINTS, f64::min);
        let limit = (limit * scale).floor() as usize;
        if limit == 0 {
            return None;
        }
        let shot = std::array::from_fn::<_, 4, _>(|corner| Samples {
            side: shot,
            corner,
            scale,
        });
        let board = std::array::from_fn::<_, 4, _>(|corner| BoardCorner {
            window,
            width: f64::from(board.picture.width) / board.scale,
            height: f64::from(board.picture.height) / board.scale,
            corner,
            scale,
        });
        let cutouts = std::array::from_fn(|corner| {
            let shot = &shot[corner];
            (shot.at(0, 0)[3] < HALF_ALPHA && shot.at(limit - 1, limit - 1)[3] == 255)
                .then(|| Cutout::read(&board[corner], shot, limit))
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

/// A corner of the board's picture, with the origin reflected to the tip of
/// the corner, as the window's shape draws it.
struct BoardCorner<'a> {
    window: &'a WindowShape,
    /// The picture's size in points.
    width: f64,
    height: f64,
    corner: usize,
    /// Pixels to the point of the grid the corner is read on.
    scale: f64,
}

impl BoardCorner<'_> {
    /// Whether the window covers the point `x` and `y` grid pixels from the
    /// tip of the corner along the two edges: its middle when `middle` is
    /// 0.5, the grid pixel's corner nearest the tip when it is 0.
    fn covers(&self, x: usize, y: usize, middle: f64) -> bool {
        let x = (x as f64 + middle) / self.scale;
        let y = (y as f64 + middle) / self.scale;
        let x = if self.corner & 1 != 0 {
            self.width - x
        } else {
            x
        };
        let y = if self.corner >= 2 { self.height - y } else { y };
        self.window.contains(x, y)
    }
}

impl Cutout {
    fn read(board: &BoardCorner, shot: &Samples, limit: usize) -> Self {
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
            let board_edge = (0..length)
                .find(|d| board.covers(x + d, y + d, 0.5))
                .unwrap_or(length);
            // A pixel is whole inside the window once its corner nearest the
            // tip is, since the window's shape is convex.
            let board_inset = (board_edge..length)
                .find(|d| board.covers(x + d, y + d, 0.0))
                .unwrap_or(length);
            let difference = if board_edge.abs_diff(shot_edge) > EDGE_TOLERANCE_PIXELS {
                board_edge.cmp(&shot_edge)
            } else {
                Ordering::Equal
            };
            rays.push((
                Insets {
                    board: board_inset,
                    shot: shot_inset,
                },
                difference,
            ));
        }
        let differs = rays.iter().any(|(_, difference)| difference.is_ne());
        let insets = rays.into_iter().map(|(insets, _)| insets).collect();
        Self { insets, differs }
    }
}
