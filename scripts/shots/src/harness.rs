//! The platform harnesses the checker holds: the sources of the code that
//! takes an app's shots inside the app's own tests, which `builds shots
//! harness` writes into the app's checkout for its test bundle to compile,
//! and the host tools a capture runs beside them. The program carries each
//! file, so the harness an app compiles is the one of the builds that takes
//! its shots, and the capture version covers it (builds' build_source.rs).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A file of a harness: its name and its contents.
pub(crate) type File = (&'static str, &'static str);

/// One platform's harness.
pub(crate) struct Harness {
    /// The sources the app's shots test bundle compiles.
    pub(crate) bundle: &'static [File],
    /// The tool that leases the shot display, `DISPLAY_TOOL`, with the
    /// sources it builds the display's keeper from.
    pub(crate) display: &'static [File],
}

/// The file of `Harness::display` that a shot's test runs.
pub(crate) const DISPLAY_TOOL: &str = "shot-display.py";

/// The file <name> of the harness folder, as the program carries it.
macro_rules! file {
    ($folder:literal, $name:literal) => {
        (
            $name,
            include_str!(concat!("../harness/", $folder, "/", $name)),
        )
    };
}

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
        file!("mac", "ShotWindowState.swift"),
        file!("mac", "Wait.swift"),
        file!("mac", "WindowBackdrop.swift"),
    ],
    display: &[
        file!("mac/display", "shot-display.py"),
        file!("mac/display", "ShotDisplayKeeper.swift"),
        file!("mac", "HangGuard.swift"),
        file!("mac", "NSScreen+ShotDisplayNumber.swift"),
        file!("mac", "ShotDisplay.swift"),
        file!("mac", "ShotDisplaySerial.swift"),
        file!("mac", "ShotLog.swift"),
        file!("mac", "Wait.swift"),
    ],
};

/// The `.gitignore` a written bundle folder holds: it keeps the folder out of
/// the app's commits, and marks the folder as one this command may replace.
const IGNORE: File = (
    ".gitignore",
    "# `builds shots harness` writes this folder whole, and replaces it.\n*\n",
);

#[derive(Debug, thiserror::Error)]
pub(crate) enum HarnessError {
    #[error(
        "{} holds files `builds shots harness` did not write: name a folder of its own, which it writes whole",
        .0.display()
    )]
    NotItsFolder(PathBuf),
    #[error("cannot write the harness at {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

fn write_error(path: &Path) -> impl FnOnce(std::io::Error) -> HarnessError {
    let path = path.to_path_buf();
    move |source| HarnessError::Write { path, source }
}

impl Harness {
    /// Makes the folder <into> hold the bundle's sources and nothing else.
    /// A folder that already holds them keeps its files untouched, so that a
    /// build compiles nothing again. Otherwise the new folder is written
    /// beside <into> and then takes its place, so <into> never holds a part
    /// of the sources. Fails for a folder that holds files and is not one
    /// this wrote.
    pub(crate) fn write_bundle(&self, into: &Path) -> Result<(), HarnessError> {
        let files: Vec<File> = self.bundle.iter().copied().chain([IGNORE]).collect();
        let held = held(into)?;
        if let Some(held) = &held {
            let wanted: BTreeMap<&str, &[u8]> = files
                .iter()
                .map(|(name, text)| (*name, text.as_bytes()))
                .collect();
            let same = held.len() == wanted.len()
                && held.iter().all(|(name, contents)| {
                    wanted.get(name.as_str()).copied() == contents.as_deref()
                });
            if same {
                return Ok(());
            }
            let ours = held
                .get(IGNORE.0)
                .is_some_and(|contents| contents.as_deref() == Some(IGNORE.1.as_bytes()));
            if !held.is_empty() && !ours {
                return Err(HarnessError::NotItsFolder(into.to_path_buf()));
            }
        }
        let into = std::path::absolute(into).map_err(write_error(into))?;
        let parent = into
            .parent()
            .ok_or_else(|| HarnessError::NotItsFolder(into.clone()))?;
        std::fs::create_dir_all(parent).map_err(write_error(parent))?;
        let beside = tempfile::Builder::new()
            .prefix(".harness-")
            .tempdir_in(parent)
            .map_err(write_error(parent))?;
        let fresh = beside.path().join("fresh");
        write_files(&fresh, &files)?;
        if held.is_some() {
            // The old folder goes away with `beside`.
            std::fs::rename(&into, beside.path().join("old")).map_err(write_error(&into))?;
        }
        std::fs::rename(&fresh, &into).map_err(write_error(&into))
    }

    /// The display tool and its keeper's sources in a new temporary folder,
    /// which goes away when the value is dropped.
    pub(crate) fn write_display_tool(&self) -> Result<tempfile::TempDir, HarnessError> {
        let folder = tempfile::Builder::new()
            .prefix("builds-shot-display-tool-")
            .tempdir()
            .map_err(write_error(&std::env::temp_dir()))?;
        write_files(folder.path(), self.display)?;
        Ok(folder)
    }
}

/// Writes <files> into the folder <folder>, which it makes.
fn write_files(folder: &Path, files: &[File]) -> Result<(), HarnessError> {
    std::fs::create_dir_all(folder).map_err(write_error(folder))?;
    for (name, text) in files {
        let path = folder.join(name);
        std::fs::write(&path, text).map_err(write_error(&path))?;
    }
    Ok(())
}

/// A folder's entries by name: a file's contents, and none for an entry that
/// is no file.
type Entries = BTreeMap<String, Option<Vec<u8>>>;

/// What the folder <folder> holds; no value when there is no folder.
fn held(folder: &Path) -> Result<Option<Entries>, HarnessError> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(write_error(folder)(error)),
    };
    let mut held = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(write_error(folder))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(write_error(&path))?;
        let contents = if kind.is_file() {
            Some(std::fs::read(&path).map_err(write_error(&path))?)
        } else {
            None
        };
        held.insert(entry.file_name().to_string_lossy().into_owned(), contents);
    }
    Ok(Some(held))
}

#[cfg(test)]
#[path = "harness_tests.rs"]
mod tests;
