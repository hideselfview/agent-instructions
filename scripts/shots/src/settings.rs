//! An app's screenshot settings: the lines of its repo's `.app-build` that
//! start with `shots-`. builds reads the file's other lines and passes these
//! through; this program reads only these, and refuses a `shots-` line it
//! does not know.
//!
//! ```text
//! shots-capture mac ./app-macos/shots-capture.sh
//! shots-capture android ./app-android/shots-capture.sh
//! shots-declared mac app-macos/Shots
//! shots-declared android app-android/src/test/java/app/shots
//! shots-preview .preview-row .preview-name .preview-cell
//! shots-preview-calls N W
//! shots-unshot .keyboard
//! ```
//!
//! - `shots-capture <platform> <command>...`: the command that takes the
//!   platform's shots, run from the repo's top (capture.rs).
//! - `shots-declared <platform> <folder>`: the folder of the harness source
//!   that declares the platform's shots (declared.rs), whose `Components`
//!   folder holds each design-system component's capture file
//!   (component_files.rs).
//! - `shots-preview <row> <name> <cell>`: the CSS selectors of a component
//!   preview's rows, of a row's state name, and of the part of a row that
//!   draws the state (render/preview.rs).
//! - `shots-preview-calls <function>...`: the functions a preview page calls
//!   with a row's name as the first string argument, which the lint reads to
//!   find a preview's states without a browser (lint.rs).
//! - `shots-unshot <selector>...`: the parts of a board the app draws no
//!   counterpart for, such as an on-screen keyboard, which the picture
//!   comparison leaves out (render/read.rs).
//!
//! A board's elements of the class `bae-preview-slot` need no line: a container's
//! preview draws one as a stand-in for its content, and the comparison leaves
//! out what either side holds inside its box (render/read.rs, compare.rs).

use crate::names::Platform;
use std::path::{Path, PathBuf};

/// The file the settings are read from, at the repo's top.
pub(crate) const FILE: &str = ".app-build";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum SettingsError {
    #[error("cannot read {0}: {1}")]
    Read(PathBuf, String),
    #[error("{FILE}: the line {0:?} {1}")]
    Line(String, &'static str),
    #[error("{FILE} has no shots-preview line, which drawing the component {0} needs")]
    NoPreview(String),
}

/// How a component preview's rows are marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preview {
    pub(crate) row: String,
    pub(crate) name: String,
    pub(crate) cell: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Each platform's capture command, in the order written.
    pub(crate) capture: Vec<(Platform, Vec<String>)>,
    /// Each platform's declare folder, relative to the repo's top.
    pub(crate) declared: Vec<(Platform, String)>,
    pub(crate) preview: Option<Preview>,
    pub(crate) preview_calls: Vec<String>,
    pub(crate) unshot: Vec<String>,
}

impl Settings {
    /// The settings of the repo at <root>; none when it has no `.app-build`.
    pub(crate) fn load(root: &Path) -> Result<Self, SettingsError> {
        let path = root.join(FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SettingsError::Read(path, e.to_string())),
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Self, SettingsError> {
        let mut settings = Self::default();
        let declared =
            boards::app_build::declared(text).map_err(|e| SettingsError::Line(e.line, e.why))?;
        settings.declared = declared;
        for line in text.lines().map(str::trim) {
            let mut words = line.split_whitespace();
            let Some(keyword) = words.next() else {
                continue;
            };
            if !keyword.starts_with("shots-") {
                continue;
            }
            let bad = |why| SettingsError::Line(line.to_string(), why);
            let rest: Vec<String> = words.map(str::to_string).collect();
            match keyword {
                "shots-capture" => {
                    let [platform, value @ ..] = rest.as_slice() else {
                        return Err(bad("names no platform"));
                    };
                    let platform = Platform::parse(platform)
                        .ok_or_else(|| bad("names a platform other than mac, android or web"))?;
                    if value.is_empty() {
                        return Err(bad("names its platform and nothing else"));
                    }
                    if settings.capture.iter().any(|(p, _)| *p == platform) {
                        return Err(bad("gives a platform a second capture command"));
                    }
                    settings.capture.push((platform, value.to_vec()));
                }
                // boards::app_build reads these lines, as builds does.
                "shots-declared" => {}
                "shots-preview" => {
                    let [row, name, cell] = rest.as_slice() else {
                        return Err(bad("needs the selectors of a row, its name and its cell"));
                    };
                    let preview = Preview {
                        row: row.clone(),
                        name: name.clone(),
                        cell: cell.clone(),
                    };
                    if settings.preview.replace(preview).is_some() {
                        return Err(bad("repeats shots-preview"));
                    }
                }
                "shots-preview-calls" | "shots-unshot" => {
                    if rest.is_empty() {
                        return Err(bad("names nothing"));
                    }
                    if keyword == "shots-unshot" {
                        settings.unshot.extend(rest);
                    } else {
                        settings.preview_calls.extend(rest);
                    }
                }
                _ => return Err(bad("is no line builds shots knows")),
            }
        }
        Ok(settings)
    }

    /// The platforms that have a capture command, in name order.
    pub(crate) fn platforms(&self) -> Vec<Platform> {
        let mut platforms: Vec<Platform> = self.capture.iter().map(|(p, _)| *p).collect();
        platforms.sort();
        platforms
    }

    /// The preview markers, which drawing component <component> needs.
    pub(crate) fn preview(&self, component: &str) -> Result<&Preview, SettingsError> {
        self.preview
            .as_ref()
            .ok_or_else(|| SettingsError::NoPreview(component.to_string()))
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
