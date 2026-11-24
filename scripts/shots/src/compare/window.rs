//! What a macOS window's own frame draws that the picture comparison leaves
//! to a shape check: the outer corners and the three buttons.

use super::buttons::Buttons;
use super::corners::Corners;
use super::{PictureSide, Side};

pub(super) struct Window {
    corners: Corners,
    /// The buttons, when each side draws them in the same places. Buttons in
    /// other places stay in the picture comparison, which marks their area.
    buttons: Option<Buttons>,
}

impl Window {
    /// The window both pictures draw, when the shot is a native window; the
    /// shot's frame is `offset` from the board's.
    pub(super) fn of(board: &Side, shot: &Side, offset: (f64, f64)) -> Option<Self> {
        let corners = Corners::of(board, shot)?;
        let buttons = Buttons::of(board, shot, &corners);
        let buttons = (!buttons.differ(offset)).then_some(buttons);
        Some(Self { corners, buttons })
    }

    pub(super) fn corners_differ(&self) -> bool {
        self.corners.differ()
    }

    /// Whether the picture comparison reads a sample in the side's own frame:
    /// it lands inside the window and on no button.
    pub(super) fn compares(&self, picture: PictureSide, side: &Side, x: f64, y: f64) -> bool {
        self.corners.contains(picture, side, x, y)
            && !self
                .buttons
                .as_ref()
                .is_some_and(|buttons| buttons.contain(picture, x, y))
    }
}
