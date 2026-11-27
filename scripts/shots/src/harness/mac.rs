//! The files of the mac harness (harness.rs).

use super::{Harness, file};

pub(crate) static MAC: Harness = Harness {
    bundle: &[
        file!("mac", "AccessibilityTree.swift"),
        file!("mac", "FocusHolder.swift"),
        file!("mac", "HangGuard.swift"),
        file!("mac", "NSScreen+ShotDisplayNumber.swift"),
        file!("mac", "ShotBoardSizes.swift"),
        file!("mac", "ShotDisplay.swift"),
        file!("mac", "ShotDisplayLease.swift"),
        file!("mac", "ShotDisplaySerial.swift"),
        file!("mac", "ShotElementList.swift"),
        file!("mac", "ShotError.swift"),
        file!("mac", "ShotImage.swift"),
        file!("mac", "ShotLog.swift"),
        file!("mac", "ShotLook.swift"),
        file!("mac", "ShotRenderer+Window.swift"),
        file!("mac", "ShotRenderer.swift"),
        file!("mac", "ShotRequest.swift"),
        file!("mac", "ShotRunner.swift"),
        file!("mac", "ShotScene.swift"),
        file!("mac", "ShotSheetShadow.swift"),
        file!("mac", "ShotWindowState.swift"),
        file!("mac", "Wait.swift"),
        file!("mac", "WindowBackdrop.swift"),
    ],
    display: Some(&[
        file!("mac/display", "shot-display.py"),
        file!("mac/display", "ShotDisplayKeeper.swift"),
        file!("mac", "HangGuard.swift"),
        file!("mac", "NSScreen+ShotDisplayNumber.swift"),
        file!("mac", "ShotDisplay.swift"),
        file!("mac", "ShotDisplaySerial.swift"),
        file!("mac", "ShotLog.swift"),
        file!("mac", "Wait.swift"),
    ]),
};
