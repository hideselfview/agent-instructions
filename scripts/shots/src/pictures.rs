//! The picture folder (`--pictures`): the board pictures earlier runs drew,
//! kept by the hash of everything each picture depends on (design.rs), so a
//! run draws only the boards whose hash the folder lacks. builds passes its
//! folder on the build volume and keeps it under a cap, deleting the
//! pictures used least recently; a run sets a picture's time when it uses it.
//!
//! A board picture is `boards/<hash>.png` with its element list
//! `boards/<hash>.json`. A run writes each file under a hidden temporary name
//! and renames it into place, the element list before the picture, so a
//! picture whose element list is missing is never there.

use std::path::{Path, PathBuf};

/// A picture folder.
pub(crate) struct Pictures {
    dir: PathBuf,
}

/// The folder of the board pictures in a picture folder.
const BOARDS: &str = "boards";

impl Pictures {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
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
        std::fs::File::options()
            .write(true)
            .open(&png)
            .and_then(|file| file.set_modified(std::time::SystemTime::now()))
            .map_err(|e| format!("cannot mark {} used: {e}", png.display()))?;
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
