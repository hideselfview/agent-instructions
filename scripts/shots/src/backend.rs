//! What differs between platforms in the shots checker. Each platform has one
//! `Backend` that holds its facts: the words and extensions of its files, how
//! its harness source declares shots, the harness it gives apps, how a board
//! is read to compare with its shots, and what the comparison allows. No other
//! module matches on `Platform`; each asks the platform's backend (`of`).

use crate::declared::{Declared, DeclaredError};
use crate::harness::Harness;
use crate::names::{App, Platform};
use std::path::Path;

/// How the board reader reads a board for a platform.
#[derive(Clone, Copy)]
pub(crate) struct Reading {
    /// Whether a Chrome role of a node is a window over the page.
    pub(crate) window_role: fn(&str) -> bool,
    /// Whether the windows read before the page (as TalkBack moves into a
    /// window that opens) rather than after it, in the order the page opens
    /// them.
    pub(crate) windows_before_page: bool,
    /// Whether a button that opens a menu or list reads as a pop-up. Compose
    /// cannot say that of a button.
    pub(crate) menu_button_is_popup: bool,
    /// Whether the platform's shots list the windows over the screen. When
    /// not, only the board's list says what covers what.
    pub(crate) shot_lists_windows: bool,
    /// The value a board writes for a secure field holding <typed>.
    pub(crate) secure_value: fn(&str) -> String,
}

/// What the comparison allows a platform's shots.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Allowances {
    /// How far, in points, a box's width or place may differ because the
    /// platform's text rounds each glyph's advance to a whole pixel. None
    /// when the platform's text does not round.
    pub(crate) glyph_advance_pixels: Option<f64>,
    /// Whether a title text's box spans the title bar, so that a board's text
    /// box is the line it sits on.
    pub(crate) spanning_title_box: bool,
    /// Whether the shots draw a window whose corners are compared.
    pub(crate) window_corners: bool,
}

/// One platform's way of declaring shots, and of reading a board to compare
/// with them.
pub(crate) trait Backend {
    /// The word in a shot's file name: `<name>@<file word>.png`.
    fn file_word(&self) -> &'static str;
    /// Whether the platform's harness source holds capture code that declares
    /// its shots and takes them with the app's `shots-capture` command,
    /// rather than a fixture file that declares each shot by its name and a
    /// `shots-stage` command that serves its page.
    fn holds_capture_code(&self) -> bool;
    /// The shots of <app> that one harness file or fixture declares, read
    /// from its source with no build.
    fn declared_in(&self, file: &Path, app: &App) -> Result<Vec<Declared>, DeclaredError>;
    /// The extension of the files that declare shots: a harness's source
    /// files, or a fixture's.
    fn declaring_extension(&self) -> &'static str;
    /// Whether the capture command also gets each `BUILDS_SHOTS_*` variable
    /// as `TEST_RUNNER_BUILDS_SHOTS_*`, which `xcodebuild test` passes on to
    /// its test runner.
    fn test_runner_copies(&self) -> bool;
    /// The harness of the platform that the checker holds, for an app's own
    /// tests to take its shots with.
    fn harness(&self) -> Option<&'static Harness>;
    fn reading(&self) -> Reading;
    fn allowances(&self) -> Allowances;
}

/// The backend of <platform>.
pub(crate) fn of(platform: Platform) -> &'static dyn Backend {
    match platform {
        Platform::Mac => &Mac,
        Platform::Android => &Android,
        Platform::Web => &Web,
    }
}

struct Mac;
struct Android;
struct Web;

impl Backend for Mac {
    fn file_word(&self) -> &'static str {
        "macos"
    }
    fn holds_capture_code(&self) -> bool {
        true
    }
    fn declaring_extension(&self) -> &'static str {
        "swift"
    }
    fn declared_in(&self, file: &Path, app: &App) -> Result<Vec<Declared>, DeclaredError> {
        crate::declared::declared_in_swift(file, app)
    }
    fn test_runner_copies(&self) -> bool {
        true
    }
    fn harness(&self) -> Option<&'static Harness> {
        Some(&crate::harness::MAC)
    }
    fn reading(&self) -> Reading {
        Reading {
            window_role: |role| role == "menu",
            windows_before_page: false,
            menu_button_is_popup: true,
            shot_lists_windows: true,
            // Mac comparisons keep whether the field is filled, because
            // native secure controls report different numbers of bullets.
            secure_value: |typed| {
                if typed.is_empty() {
                    String::new()
                } else {
                    "\u{F79A}".to_string()
                }
            },
        }
    }
    fn allowances(&self) -> Allowances {
        Allowances {
            glyph_advance_pixels: None,
            spanning_title_box: true,
            window_corners: true,
        }
    }
}

impl Backend for Android {
    fn file_word(&self) -> &'static str {
        "android"
    }
    fn holds_capture_code(&self) -> bool {
        true
    }
    fn declaring_extension(&self) -> &'static str {
        "kt"
    }
    fn declared_in(&self, file: &Path, app: &App) -> Result<Vec<Declared>, DeclaredError> {
        crate::declared::declared_in_kotlin(file, app)
    }
    fn test_runner_copies(&self) -> bool {
        false
    }
    fn harness(&self) -> Option<&'static Harness> {
        Some(&crate::harness::ANDROID)
    }
    fn reading(&self) -> Reading {
        Reading {
            window_role: |role| matches!(role, "menu" | "dialog"),
            windows_before_page: true,
            menu_button_is_popup: false,
            // The Android harness records no window in its list (it writes
            // Compose roles, never menu or dialog).
            shot_lists_windows: false,
            secure_value: plain_bullets,
        }
    }
    fn allowances(&self) -> Allowances {
        Allowances {
            // Android's native text rounds each glyph advance to a whole
            // pixel, so each glyph sets a width or a place off by up to half
            // a pixel.
            glyph_advance_pixels: Some(0.5),
            spanning_title_box: false,
            window_corners: false,
        }
    }
}

impl Backend for Web {
    fn file_word(&self) -> &'static str {
        "web"
    }
    fn holds_capture_code(&self) -> bool {
        false
    }
    fn declaring_extension(&self) -> &'static str {
        "json"
    }
    fn declared_in(&self, file: &Path, app: &App) -> Result<Vec<Declared>, DeclaredError> {
        crate::declared::declared_by_name(file, app)
    }
    fn test_runner_copies(&self) -> bool {
        false
    }
    fn harness(&self) -> Option<&'static Harness> {
        None
    }
    fn reading(&self) -> Reading {
        Reading {
            window_role: |_| false,
            windows_before_page: false,
            menu_button_is_popup: false,
            shot_lists_windows: true,
            secure_value: plain_bullets,
        }
    }
    fn allowances(&self) -> Allowances {
        Allowances {
            glyph_advance_pixels: None,
            spanning_title_box: false,
            window_corners: false,
        }
    }
}

/// One bullet for each typed character, as Compose and Chrome write one.
fn plain_bullets(typed: &str) -> String {
    typed.chars().map(|_| '\u{2022}').collect()
}

/// The platform whose word in a shot's file name is <word>.
pub(crate) fn from_file_word(word: &str) -> Option<Platform> {
    Platform::ALL
        .into_iter()
        .find(|platform| of(*platform).file_word() == word)
}
