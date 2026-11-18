//! The shots the harnesses declare, read from their source in the folder the
//! app's `shots-declared` line gives each platform: on android, the first
//! argument of each Kotlin `shot("…")` and `component("…")` call; on mac,
//! each Swift string literal of board form; on web, the name of each fixture
//! file `<name>.json`. Reading source needs no build of the app, so the
//! commit hook can run it.

use crate::names::{Name, NameError, Platform};
use regex::Regex;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum DeclaredError {
    #[error("{0}: {1}")]
    Name(PathBuf, NameError),
    #[error("cannot read {0}: {1}")]
    Read(PathBuf, std::io::Error),
    #[error(
        "{0}: a scene shown in its window is written `ShotScene(id: \"<board name>\", …, frame: .window(…)) {{ … }}` so the tool can tell it needs the window"
    )]
    WindowScene(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declared {
    pub(crate) name: Name,
    pub(crate) file: PathBuf,
    /// The scene is shown in its window, which takes the focus, so only a run
    /// given `--windows` takes it. Declared in Swift by `frame: .window(…)`.
    pub(crate) needs_window: bool,
}

/// A reader of a harness's source: each shot name it declares, and whether
/// the shot is shown in a window.
type Reader = fn(&str) -> Vec<(String, bool)>;

/// Every shot the harnesses in <folders> (each platform's declare folder,
/// relative to <root>) declare, in name order, each once.
pub(crate) fn read(
    root: &Path,
    folders: &[(Platform, String)],
) -> Result<Vec<Declared>, DeclaredError> {
    let mut all = Vec::new();
    for (platform, directory) in folders {
        // A web shot's fixture declares it by its name; a harness's source
        // declares its shots inside.
        let (extension, find): (&str, Option<Reader>) = match platform {
            Platform::Android => ("kt", Some(kotlin)),
            Platform::Mac => ("swift", Some(swift)),
            Platform::Web => ("json", None),
        };
        let directory = root.join(directory);
        let entries =
            std::fs::read_dir(&directory).map_err(|e| DeclaredError::Read(directory.clone(), e))?;
        for entry in entries {
            let path = entry
                .map_err(|e| DeclaredError::Read(directory.clone(), e))?
                .path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            // A test of the harness writes its own boards; it declares none.
            if path.extension().and_then(|e| e.to_str()) != Some(extension)
                || stem.ends_with("Test")
                || stem.ends_with("Tests")
            {
                continue;
            }
            let Some(find) = find else {
                let name = Name::parse(stem).map_err(|e| DeclaredError::Name(path.clone(), e))?;
                all.push(Declared {
                    name,
                    file: path.clone(),
                    needs_window: false,
                });
                continue;
            };
            let source =
                std::fs::read_to_string(&path).map_err(|e| DeclaredError::Read(path.clone(), e))?;
            let source = if extension == "swift" {
                without_line_comments(&source)
            } else {
                source
            };
            let found = find(&source);
            if extension == "swift"
                && window_frames(&source) != found.iter().filter(|f| f.1).count()
            {
                return Err(DeclaredError::WindowScene(path));
            }
            for (text, needs_window) in found {
                let name = Name::parse(&text).map_err(|e| DeclaredError::Name(path.clone(), e))?;
                all.push(Declared {
                    name,
                    file: path.clone(),
                    needs_window,
                });
            }
        }
    }
    all.sort_by(|a, b| a.name.cmp(&b.name).then(a.file.cmp(&b.file)));
    all.dedup_by(|a, b| a.name == b.name);
    Ok(all)
}

/// The first string argument of each `shot(` and `component(` call.
fn kotlin(source: &str) -> Vec<(String, bool)> {
    let call = Regex::new(r#"\b(?:shot|component)\(\s*"([^"]*)""#).unwrap();
    call.captures_iter(source)
        .map(|c| (c[1].to_string(), false))
        .collect()
}

/// Each string literal that begins like a Mac board name, with whether it is
/// the id of a `ShotScene(…)` call whose arguments give `frame: .window(`.
fn swift(source: &str) -> Vec<(String, bool)> {
    let literal = Regex::new(r#""(mac\.[^"\s]*)""#).unwrap();
    let in_window = Regex::new(r#"ShotScene\(\s*id:\s*"(mac\.[^"\s]*)"[^{}]*?\)\s*\{"#).unwrap();
    let windowed: Vec<&str> = in_window
        .captures_iter(source)
        .filter(|c| c[0].contains("frame: .window("))
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    literal
        .captures_iter(source)
        .map(|c| (c[1].to_string(), windowed.contains(&&c[1])))
        .collect()
}

/// The source without its whole-line comments, which may describe a scene's
/// form in the very words the readers look for.
fn without_line_comments(source: &str) -> String {
    source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// How many times the source writes `frame: .window(`, each of which the
/// reading above must have tied to a scene id.
fn window_frames(source: &str) -> usize {
    source.matches("frame: .window(").count()
}

#[cfg(test)]
#[path = "declared_tests.rs"]
mod tests;
