//! The shots the harnesses declare, read from their source in the folder the
//! app's `shots-declared` line gives each platform: on android, the first
//! argument of each Kotlin `shot("…")` and `component("…")` call; on mac,
//! each Swift string literal of board form; on web, the name of each fixture
//! file `<name>.json`. The folder's `Components` folder holds each
//! design-system component's capture file (component_files.rs), whose shots
//! are read the same way. Reading source needs no build of the app, so the
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
    #[error(
        "{0}: a scene that takes the keyboard focus is written `ShotScene(id: \"<board name>\", …, takesFocus: true) {{ … }}` so the tool can tell it takes the focus"
    )]
    FocusScene(PathBuf),
    #[error("{0}: the file's name is not UTF-8")]
    NotUtf8(PathBuf),
    #[error("{first} and {second} both declare {name}; one file declares each shot")]
    Twice {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },
    #[error(
        "{} has no shots-declared line, so there is no harness to read",
        crate::settings::FILE
    )]
    NoHarness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    pub name: Name,
    pub file: PathBuf,
    /// The scene is shown in its window, drawn on a display of the Mac's.
    /// Declared in Swift by `frame: .window(…)` among the arguments of its
    /// `ShotScene(` call.
    pub in_window: bool,
    /// The scene's window must be drawn by an app that holds the keyboard
    /// focus, so only a run given `--windows` takes it. Declared in Swift by
    /// `takesFocus: true` among the same arguments.
    pub takes_focus: bool,
}

/// What a harness's source says of how a shot is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Marks {
    in_window: bool,
    takes_focus: bool,
}

/// A reader of a harness's source: each shot name it declares, and how the
/// source says the shot is drawn.
type Reader = fn(&str) -> Vec<(String, Marks)>;

/// The folder under a declare folder that holds each design-system
/// component's capture file (component_files.rs). Its files declare shots as
/// the declare folder's own do.
pub(crate) const COMPONENTS: &str = "Components";

/// How <platform>'s harness declares its shots: the extension of its files,
/// and the reader of its source. A web shot's fixture declares it by its
/// name, so web has no reader.
fn harness(platform: Platform) -> (&'static str, Option<Reader>) {
    match platform {
        Platform::Android => ("kt", Some(kotlin)),
        Platform::Mac => ("swift", Some(swift)),
        Platform::Web => ("json", None),
    }
}

/// The extension of <platform>'s harness source, none for a platform whose
/// shots are declared by their fixture files, which hold no capture code.
pub(crate) fn source_extension(platform: Platform) -> Option<&'static str> {
    match harness(platform) {
        (extension, Some(_)) => Some(extension),
        (_, None) => None,
    }
}

/// The name of the file at <path> without its extension.
fn stem(path: &Path) -> Result<&str, DeclaredError> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| DeclaredError::NotUtf8(path.to_path_buf()))
}

/// A harness file: its path, and its name without its extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HarnessFile {
    pub(crate) path: PathBuf,
    pub(crate) stem: String,
}

/// The harness files of <platform> in <directory>, each a file with the
/// harness's extension that is no test of the harness: a test writes its own
/// boards and declares none.
fn harness_files(directory: &Path, platform: Platform) -> Result<Vec<HarnessFile>, DeclaredError> {
    harness_among(files(directory)?, platform)
}

/// The harness files of <platform> in the `Components` folder of the declare
/// folder <declare_folder>, which hold the components' capture files; none
/// when it has no `Components` folder.
pub(crate) fn capture_files(
    declare_folder: &Path,
    platform: Platform,
) -> Result<Vec<HarnessFile>, DeclaredError> {
    match files(&declare_folder.join(COMPONENTS)) {
        Err(DeclaredError::Read(_, e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Vec::new())
        }
        paths => harness_among(paths?, platform),
    }
}

/// The harness files of <platform> among <paths>.
fn harness_among(
    paths: Vec<PathBuf>,
    platform: Platform,
) -> Result<Vec<HarnessFile>, DeclaredError> {
    let (extension, _) = harness(platform);
    let mut harness = Vec::new();
    for path in paths {
        if path.extension().and_then(|e| e.to_str()) != Some(extension) {
            continue;
        }
        let stem = stem(&path)?.to_string();
        if !stem.ends_with("Test") && !stem.ends_with("Tests") {
            harness.push(HarnessFile { path, stem });
        }
    }
    Ok(harness)
}

/// Every shot the harnesses in <folders> (each platform's declare folder,
/// relative to <root>) declare, in name order. A declare folder's files and
/// those of its `Components` folder declare shots. A file may write a shot's
/// name more than once; two files that declare one shot fail the read, since
/// both would write its picture.
pub(crate) fn read(
    root: &Path,
    folders: &[(Platform, String)],
) -> Result<Vec<Declared>, DeclaredError> {
    if folders.is_empty() {
        return Err(DeclaredError::NoHarness);
    }
    let mut all = Vec::new();
    for (platform, directory) in folders {
        let directory = root.join(directory);
        let mut harness = harness_files(&directory, *platform)?;
        harness.extend(capture_files(&directory, *platform)?);
        for file in harness {
            all.extend(declared_in(*platform, &file.path)?);
        }
    }
    all.sort_by(|a, b| a.name.cmp(&b.name).then(a.file.cmp(&b.file)));
    all.dedup_by(|a, b| a.name == b.name && a.file == b.file);
    if let Some(pair) = all.windows(2).find(|pair| pair[0].name == pair[1].name) {
        return Err(DeclaredError::Twice {
            name: pair[0].name.to_string(),
            first: pair[0].file.clone(),
            second: pair[1].file.clone(),
        });
    }
    Ok(all)
}

/// The paths of the entries of <directory>.
fn files(directory: &Path) -> Result<Vec<PathBuf>, DeclaredError> {
    std::fs::read_dir(directory)
        .and_then(|entries| entries.map(|e| e.map(|e| e.path())).collect())
        .map_err(|e| DeclaredError::Read(directory.to_path_buf(), e))
}

/// The shots the harness file <path> of <platform> declares, in the order
/// it writes them: a source file's shots, or the one shot a web fixture
/// file declares by its name.
pub(crate) fn declared_in(platform: Platform, path: &Path) -> Result<Vec<Declared>, DeclaredError> {
    let (extension, find) = harness(platform);
    let Some(find) = find else {
        let name =
            Name::parse(stem(path)?).map_err(|e| DeclaredError::Name(path.to_path_buf(), e))?;
        return Ok(vec![Declared {
            name,
            file: path.to_path_buf(),
            in_window: false,
            takes_focus: false,
        }]);
    };
    let source =
        std::fs::read_to_string(path).map_err(|e| DeclaredError::Read(path.to_path_buf(), e))?;
    let source = if extension == "swift" {
        without_line_comments(&source)
    } else {
        source
    };
    let found = find(&source);
    if extension == "swift" {
        let count = |mark: fn(&Marks) -> bool| found.iter().filter(|f| mark(&f.1)).count();
        if window_frames(&source) != count(|m| m.in_window) {
            return Err(DeclaredError::WindowScene(path.to_path_buf()));
        }
        if focus_markers(&source) != count(|m| m.takes_focus) {
            return Err(DeclaredError::FocusScene(path.to_path_buf()));
        }
    }
    found
        .into_iter()
        .map(|(text, marks)| {
            Ok(Declared {
                name: Name::parse(&text).map_err(|e| DeclaredError::Name(path.to_path_buf(), e))?,
                file: path.to_path_buf(),
                in_window: marks.in_window,
                takes_focus: marks.takes_focus,
            })
        })
        .collect()
}

/// The first string argument of each `shot(` and `component(` call.
fn kotlin(source: &str) -> Vec<(String, Marks)> {
    let call = Regex::new(r#"\b(?:shot|component)\(\s*"([^"]*)""#).unwrap();
    call.captures_iter(source)
        .map(|c| (c[1].to_string(), Marks::default()))
        .collect()
}

/// Each string literal that begins like a Mac board name, with whether it is
/// the id of a `ShotScene(…)` call whose arguments give `frame: .window(` and
/// `takesFocus: true`.
fn swift(source: &str) -> Vec<(String, Marks)> {
    let literal = Regex::new(r#""(mac\.[^"\s]*)""#).unwrap();
    let scene = Regex::new(r#"ShotScene\(\s*id:\s*"(mac\.[^"\s]*)"[^{}]*?\)\s*\{"#).unwrap();
    let calls: Vec<(&str, Marks)> = scene
        .captures_iter(source)
        .map(|c| {
            let marks = Marks {
                in_window: c[0].contains("frame: .window("),
                takes_focus: c[0].contains("takesFocus: true"),
            };
            (c.get(1).unwrap().as_str(), marks)
        })
        .collect();
    literal
        .captures_iter(source)
        .map(|c| {
            let mut marks = Marks::default();
            for (id, called) in &calls {
                if *id == &c[1] {
                    marks.in_window |= called.in_window;
                    marks.takes_focus |= called.takes_focus;
                }
            }
            (c[1].to_string(), marks)
        })
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

/// How many times the source writes `takesFocus: true` outside a string
/// literal, each of which the reading above must have tied to a scene id.
fn focus_markers(source: &str) -> usize {
    let literal = Regex::new(r#""[^"\n]*""#).unwrap();
    literal
        .replace_all(source, "\"\"")
        .matches("takesFocus: true")
        .count()
}

#[cfg(test)]
#[path = "declared_tests.rs"]
mod tests;
