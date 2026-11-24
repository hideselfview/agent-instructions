//! The "SF Mono" family that boards whose mono stack names it draw mono
//! text in. The Mac does not
//! install it as a family that a browser can name: its mono system font is
//! the hidden ".SF NS Mono", and Chrome draws the boards' mono stack in Menlo,
//! which is narrower (7.225 pt a character against the app's 7.43). The Mac
//! ships the family inside Terminal.app. Servers of boards read those files
//! when a page asks for them and describe them to the page with `@font-face`;
//! nothing here copies a font into a repo.

use crate::{Error, Result};
use std::path::Path;

/// The folder of Terminal.app that holds the SF Mono files.
pub const TERMINAL_FONTS: &str =
    "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts";

/// The family name that a board's mono stack can name to draw the files
/// (bae's does: `ui-monospace, "SF Mono", Menlo, monospace`).
pub const FAMILY: &str = "SF Mono";

/// The URL path a font file is served under, before its file name.
pub const URL_PREFIX: &str = "/fonts/sf-mono/";

/// The files of the family: name, CSS weight, italic.
pub const FACES: [(&str, u16, bool); 12] = [
    ("SF-Mono-Light.otf", 300, false),
    ("SF-Mono-LightItalic.otf", 300, true),
    ("SF-Mono-Regular.otf", 400, false),
    ("SF-Mono-RegularItalic.otf", 400, true),
    ("SF-Mono-Medium.otf", 500, false),
    ("SF-Mono-MediumItalic.otf", 500, true),
    ("SF-Mono-Semibold.otf", 600, false),
    ("SF-Mono-SemiboldItalic.otf", 600, true),
    ("SF-Mono-Bold.otf", 700, false),
    ("SF-Mono-BoldItalic.otf", 700, true),
    ("SF-Mono-Heavy.otf", 800, false),
    ("SF-Mono-HeavyItalic.otf", 800, true),
];

/// Whether <name> is the name of a file of the family: the only names a
/// server reads from the folder (`routes::route`).
pub(crate) fn is_font_file(name: &str) -> bool {
    FACES.iter().any(|(file, ..)| *file == name)
}

/// The `@font-face` rules for the files of the family in <dir>, each loading
/// from `URL_PREFIX` on the server that serves the page. A page that is a
/// file, as `builds design render` prints, cannot load them, because the URLs
/// are server paths, and draws the mono stack's next family. Empty when <dir>
/// is no folder, as on a host that is not a Mac; an error when <dir> exists
/// and a file of the family is missing or cannot be opened, which would draw
/// Menlo's narrower mono text with nothing saying why.
pub fn font_face_css(dir: &Path) -> Result<String> {
    if !dir.is_dir() {
        return Ok(String::new());
    }
    let mut rules = Vec::new();
    for (file, weight, italic) in FACES {
        let path = dir.join(file);
        let opened = std::fs::File::open(&path).and_then(|opened| opened.metadata());
        match opened {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(Error::new(format!("{} is not a file", path.display()))),
            Err(error) => {
                return Err(Error::new(format!(
                    "the SF Mono file {} cannot be read: {error}",
                    path.display()
                )));
            }
        }
        let style = if italic { "italic" } else { "normal" };
        rules.push(format!(
            "@font-face {{ font-family: \"{FAMILY}\"; font-weight: {weight}; font-style: {style}; src: url(\"{URL_PREFIX}{file}\") format(\"opentype\"); }}"
        ));
    }
    Ok(rules.join("\n"))
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod tests;
