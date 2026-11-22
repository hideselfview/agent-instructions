//! The picture folder (`--pictures`): the board pictures earlier runs drew,
//! kept by the hash of everything each picture depends on (design.rs), so a
//! run draws only the boards whose hash the folder lacks; and the app's
//! shots earlier runs took, kept by the app source tree builds names
//! (`--app-tree`), so a run of an app tree the folder holds every selected
//! shot of takes none again. builds passes its
//! folder on the build volume and keeps it under a cap, deleting the
//! pictures used least recently; a run sets a picture's time when it uses it.
//!
//! A board picture is `boards/<hash>.png` with its element list
//! `boards/<hash>.json`, and an app's shot is
//! `<platform>/<app tree>/<name>@<platform>.png` with its element list. Once
//! a run has kept every shot a prefix selects, it marks the prefix taken in
//! `<platform>/<app tree>/.taken/<prefix>@windows` (or `@no-windows` for a
//! run without `--windows`), so builds knows without the harness sources
//! which runs need no app built. A run writes each file under a hidden temporary name
//! and renames it into place, the element list before the picture, so a
//! picture whose element list is missing is never there.

use crate::names::{Name, Platform, Prefix};
use crate::{CaptureMarkError, CaptureMarks};
use std::path::{Path, PathBuf};

/// The folder that holds screenshots captured for one app tree and platform.
pub fn app_picture_tree(root: &Path, platform: Platform, tree: &str) -> PathBuf {
    root.join(platform.word()).join(tree)
}

/// A picture folder.
pub(crate) struct Pictures {
    dir: PathBuf,
    /// The time the run marks each picture it takes used: one time, read as
    /// the run starts.
    now: std::time::SystemTime,
}

/// The folder of the board pictures in a picture folder.
const BOARDS: &str = "boards";

impl Pictures {
    pub(crate) fn new(dir: PathBuf, now: std::time::SystemTime) -> Self {
        Self { dir, now }
    }

    /// Marks the picture <png> used at the run's time.
    fn mark_used(&self, png: &Path) -> Result<(), String> {
        std::fs::File::options()
            .write(true)
            .open(png)
            .and_then(|file| file.set_modified(self.now))
            .map_err(|e| format!("cannot mark {} used: {e}", png.display()))
    }

    /// The picture and element list kept for the board hash <hash>.
    fn board(&self, hash: &str) -> (PathBuf, PathBuf) {
        let boards = self.dir.join(BOARDS);
        (
            boards.join(format!("{hash}.png")),
            boards.join(format!("{hash}.json")),
        )
    }

    /// Copies the board picture kept for <hash> into <out> as `<shot>.png` and
    /// `<shot>.json`, and marks it used; false when the folder keeps none, or
    /// builds deleted it meanwhile.
    pub(crate) fn take_board(&self, hash: &str, shot: &str, out: &Path) -> Result<bool, String> {
        let (png, json) = self.board(hash);
        if !png.is_file() || !json.is_file() {
            return Ok(false);
        }
        std::fs::create_dir_all(out)
            .map_err(|e| format!("cannot prepare {}: {e}", out.display()))?;
        for (from, to) in [
            (&json, out.join(format!("{shot}.json"))),
            (&png, out.join(format!("{shot}.png"))),
        ] {
            match std::fs::copy(from, &to) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !from.exists() => {
                    return Ok(false);
                }
                Err(e) => {
                    return Err(format!(
                        "cannot copy {} to {}: {e}",
                        from.display(),
                        to.display()
                    ));
                }
            }
        }
        self.mark_used(&png)?;
        Ok(true)
    }

    /// Keeps the board picture drawn into <out> as `<shot>.png` and
    /// `<shot>.json` under <hash>.
    pub(crate) fn keep_board(&self, hash: &str, shot: &str, out: &Path) -> Result<(), String> {
        let (png, json) = self.board(hash);
        let boards = self.dir.join(BOARDS);
        std::fs::create_dir_all(&boards)
            .map_err(|e| format!("cannot prepare {}: {e}", boards.display()))?;
        place(&out.join(format!("{shot}.json")), &json)?;
        place(&out.join(format!("{shot}.png")), &png)
    }
}

impl Pictures {
    /// Copies the shots <names> of <platform> kept for the app source tree
    /// <tree> into <out>, and marks them used, when the folder keeps every
    /// one; false, copying nothing, when it lacks one.
    pub(crate) fn take_shots(
        &self,
        platform: Platform,
        tree: &str,
        names: &[Name],
        out: &Path,
    ) -> Result<bool, String> {
        let dir = app_picture_tree(&self.dir, platform, tree);
        let files: Vec<String> = names
            .iter()
            .flat_map(|name| [name.file_name("json"), name.file_name("png")])
            .collect();
        if !files.iter().all(|file| dir.join(file).is_file()) {
            return Ok(false);
        }
        std::fs::create_dir_all(out)
            .map_err(|e| format!("cannot prepare {}: {e}", out.display()))?;
        for file in &files {
            copy(&dir.join(file), &out.join(file))?;
        }
        for name in names {
            self.mark_used(&dir.join(name.file_name("png")))?;
        }
        Ok(true)
    }

    /// Marks each of <prefixes> taken for <platform>'s app source tree
    /// <tree>, by a run with or without the shots that take the keyboard focus
    /// (<windows>): the folder keeps every shot each selects.
    pub(crate) fn mark_taken(
        &self,
        platform: Platform,
        tree: &str,
        prefixes: &[Prefix],
        windows: bool,
    ) -> Result<(), CaptureMarkError> {
        CaptureMarks::of(&app_picture_tree(&self.dir, platform, tree)).write(prefixes, windows)
    }

    /// Keeps the shots <names> of <platform> taken into <out> for the app
    /// source tree <tree>.
    pub(crate) fn keep_shots(
        &self,
        platform: Platform,
        tree: &str,
        names: &[Name],
        out: &Path,
    ) -> Result<(), String> {
        let dir = app_picture_tree(&self.dir, platform, tree);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot prepare {}: {e}", dir.display()))?;
        for name in names {
            for extension in ["json", "png"] {
                let file = name.file_name(extension);
                place(&out.join(&file), &dir.join(&file))?;
            }
        }
        Ok(())
    }
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))
}

/// Copies <from> to <to> through a hidden temporary file beside <to>, which
/// it then renames to <to>.
fn place(from: &Path, to: &Path) -> Result<(), String> {
    let name = to
        .file_name()
        .expect("a picture file has a name")
        .to_string_lossy();
    let temporary = to.with_file_name(format!(".{name}.{}", std::process::id()));
    copy(from, &temporary)?;
    std::fs::rename(&temporary, to).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!(
            "cannot move {} to {}: {e}",
            temporary.display(),
            to.display()
        )
    })
}

#[cfg(test)]
#[path = "pictures_tests.rs"]
mod tests;
