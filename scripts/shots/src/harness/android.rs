//! The files of the android harness (harness.rs).

use super::{Harness, file};

pub(crate) static ANDROID: Harness = Harness {
    bundle: &[
        file!("android", "BoardShot.kt"),
        file!("android", "ComposeView.kt"),
        file!("android", "ElementList.kt"),
        file!("android", "ElementValues.kt"),
        file!("android", "EmbeddedViews.kt"),
        file!("android", "ReadingOrder.kt"),
        file!("android", "ShotBoardSizes.kt"),
        file!("android", "ShotLook.kt"),
        file!("android", "ShotRequest.kt"),
        file!("android", "ShotSettle.kt"),
        file!("android", "TestAnimatorScale.kt"),
        file!("android", "WindowOverScreen.kt"),
    ],
    display: None,
};
