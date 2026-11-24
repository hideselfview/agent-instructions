//! The "SF Mono" family the boards draw mono text in. The Mac does not
//! install it as a family that a browser can name: its mono system font is
//! the hidden ".SF NS Mono", and Chrome draws the boards' mono stack in Menlo,
//! which is narrower (7.225 pt a character against the app's 7.43). The Mac
//! ships the family inside Terminal.app. Servers of boards read those files
//! when a page asks for them and describe them to the page with `@font-face`;
//! nothing here copies a font into a repo.

use std::path::Path;

/// The folder of Terminal.app that holds the SF Mono files.
pub const TERMINAL_FONTS: &str =
    "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts";

/// The family name the boards' mono stack names.
pub const FAMILY: &str = "SF Mono";

/// The URL path a font file is served under, before its file name.
pub const URL_PREFIX: &str = "/fonts/sf-mono/";

/// The files of the family: name, CSS weight, italic.
const FACES: [(&str, u16, bool); 12] = [
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
/// server reads from the folder.
pub fn is_font_file(name: &str) -> bool {
    FACES.iter().any(|(file, ..)| *file == name)
}

/// The `@font-face` rules for the files of the family that <dir> holds, each
/// loading from `URL_PREFIX` on the server that serves the page. Empty when
/// <dir> holds none, as on a host that is not a Mac.
pub fn font_face_css(dir: &Path) -> String {
    FACES
        .iter()
        .filter(|(file, ..)| dir.join(file).is_file())
        .map(|(file, weight, italic)| {
            let style = if *italic { "italic" } else { "normal" };
            format!(
                "@font-face {{ font-family: \"{FAMILY}\"; font-weight: {weight}; font-style: {style}; src: url(\"{URL_PREFIX}{file}\") format(\"opentype\"); }}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The bytes of the font file <name> in <dir>; none when <name> is no file of
/// the family or <dir> lacks it.
pub fn read_font(dir: &Path, name: &str) -> std::io::Result<Option<Vec<u8>>> {
    if !is_font_file(name) {
        return Ok(None);
    }
    match std::fs::read(dir.join(name)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod tests;
