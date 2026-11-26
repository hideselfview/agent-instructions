//! An app's screenshot settings: the lines of its repo's `.app-build` that
//! start with `shots-`. builds reads the file's other lines and passes these
//! through; this program reads only these, and refuses a `shots-` line it
//! does not know.
//!
//! ```text
//! app windows avalonia
//! shots-capture mac ./app-macos/shots-capture.sh
//! shots-capture android ./app-android/shots-capture.sh
//! shots-declared mac app-macos/Shots
//! shots-declared android app-android/src/test/java/app/shots
//! shots-preview .preview-row .preview-name .preview-cell
//! shots-preview-calls N W
//! shots-unshot .keyboard
//! shots-design-only mac AppIcon Sym
//! shots-design-only android Logo.inline Logo.sheep
//! ```
//!
//! - `app <name> <platform>`: an app and the platform whose backend takes its
//!   shots (`boards::app_build`). An app named for a platform (`mac`) needs
//!   no line. Every other line below names an app by its name.
//! - `shots-capture <app> <command>...`: the command that takes the app's
//!   shots, run from the repo's top (capture.rs).
//! - `shots-declared <app> <folder>`: the folder of the harness source
//!   that declares the app's shots (declared.rs), whose `Components`
//!   folder holds each design-system component's capture file
//!   (component_files.rs).
//! - `shots-preview <row> <name> <cell>`: the CSS selectors of a component
//!   preview's rows, of a row's state name, and of the part of a row that
//!   draws the state (render/preview.rs).
//! - `shots-preview-calls <function>...`: the functions a preview page calls
//!   with a row's name as the first string argument, which the lint reads to
//!   find a preview's states without a browser (lint.rs).
//! - `shots-design-only <app> <Component>...`: the components the
//!   design draws and the app has no view for (an app icon's
//!   artwork, a glyph library), whose preview rows have no shot and never
//!   will, so the listing of component capture files leaves them out and
//!   says how many (component_files.rs). `Component.row` marks one row of
//!   the component's preview, named by its id (`Nested, off` is
//!   `nested-off`), for an app that draws the component's other rows
//!   and not that one; the other rows keep their shots.
//! - `shots-unshot <selector>...`: the parts of a board the app draws no
//!   counterpart for, such as an on-screen keyboard, which the picture
//!   comparison leaves out (render/read.rs).
//!
//! A board's elements of the class `bae-preview-slot` need no line: a container's
//! preview draws one as a stand-in for its content, and the comparison leaves
//! out what either side holds inside its box (render/read.rs, compare.rs).

use crate::names::{App, Apps};
use std::path::{Path, PathBuf};

/// A `shots-design-only` name split into its component and, for the form
/// `Component.row`, the id of one row of the component's preview.
pub(crate) fn split_design_only(name: &str) -> (&str, Option<&str>) {
    match name.split_once('.') {
        Some((component, row)) => (component, Some(row)),
        None => (name, None),
    }
}

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

/// What a line says when its app is no app of the file.
const NO_APP: &str = "names no app (an app line or a platform's word gives one)";

/// How a component preview's rows are marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preview {
    pub(crate) row: String,
    pub(crate) name: String,
    pub(crate) cell: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Settings {
    /// The apps the `app` lines name.
    pub(crate) apps: Apps,
    /// Each app's capture command, in the order written.
    pub(crate) capture: Vec<(App, Vec<String>)>,
    /// Each app's declare folder, relative to the repo's top.
    pub(crate) declared: Vec<(App, String)>,
    pub(crate) preview: Option<Preview>,
    pub(crate) preview_calls: Vec<String>,
    pub(crate) unshot: Vec<String>,
    /// Each component an app draws for the design only, in the order written.
    pub(crate) design_only: Vec<(App, String)>,
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
        let line_error = |e: boards::app_build::LineError| SettingsError::Line(e.line, e.why);
        settings.apps = Apps::read(text).map_err(line_error)?;
        settings.declared = boards::app_build::declared(text).map_err(line_error)?;
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
                    let [app, value @ ..] = rest.as_slice() else {
                        return Err(bad("names no app"));
                    };
                    let app = settings.app(app).ok_or_else(|| bad(NO_APP))?;
                    if value.is_empty() {
                        return Err(bad("names its app and nothing else"));
                    }
                    if settings.capture.iter().any(|(a, _)| *a == app) {
                        return Err(bad("gives an app a second capture command"));
                    }
                    settings.capture.push((app, value.to_vec()));
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
                "shots-design-only" => {
                    let [app, components @ ..] = rest.as_slice() else {
                        return Err(bad("names no app"));
                    };
                    let app = settings.app(app).ok_or_else(|| bad(NO_APP))?;
                    if components.is_empty() {
                        return Err(bad("names no component"));
                    }
                    for component in components {
                        match split_design_only(component) {
                            ("", _) => return Err(bad("names no component")),
                            (_, Some("")) => return Err(bad("names no row")),
                            _ => settings.design_only.push((app.clone(), component.clone())),
                        }
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

    /// The app called <name>, when an `app` line or a platform's word gives it.
    pub(crate) fn app(&self, name: &str) -> Option<App> {
        self.apps.get(name)
    }

    /// The apps that have a capture command, in name order.
    pub(crate) fn capturing_apps(&self) -> Vec<App> {
        let mut apps: Vec<App> = self.capture.iter().map(|(a, _)| a.clone()).collect();
        apps.sort();
        apps
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
