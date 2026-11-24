//! Writing a file whole: a reader of the file sees its old contents or its
//! new ones, never part of them.

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};

/// Writes <contents> to the file beside <file> with the extension `part`,
/// then renames that file onto <file> in one step. Each write has a `part`
/// file of its own, so writers of one file may run at once.
pub fn write(file: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    stage(file, contents)?.put()
}

/// `write`, with the file executable for everyone who can read it: the mode is
/// set on the `part` file, so no reader finds the file without it.
pub fn write_executable(file: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let staged = stage(file, contents)?;
    std::fs::set_permissions(&staged.part, std::fs::Permissions::from_mode(0o755))
        .context(|| format!("making {} executable", staged.part.display()))?;
    staged.put()
}

/// New contents of a file, written to the process's `part` file beside it, that
/// `put` renames onto the file and `discard` removes.
pub struct Staged {
    part: PathBuf,
    file: PathBuf,
}

/// Writes <contents> to the file beside <file> with the extension `part`
/// and leaves <file> as it is until `Staged::put`. A rename within one
/// folder needs no new disk space, so the put does not fail where writing
/// the contents could.
pub fn stage(file: &Path, contents: impl AsRef<[u8]>) -> Result<Staged> {
    let part = file.with_extension(format!("{}.part", crate::text::staged_suffix()));
    std::fs::write(&part, contents).context(|| format!("writing {}", part.display()))?;
    Ok(Staged {
        part,
        file: file.to_path_buf(),
    })
}

impl Staged {
    /// Renames the `part` file onto the file in one step.
    pub fn put(self) -> Result<()> {
        std::fs::rename(&self.part, &self.file)
            .context(|| format!("replacing {}", self.file.display()))
    }

    /// Removes the `part` file and leaves the file as it was.
    pub fn discard(self) -> Result<()> {
        std::fs::remove_file(&self.part).context(|| format!("removing {}", self.part.display()))
    }
}

/// Removes the file at <path>. Returns false if it was already gone.
pub(crate) fn remove_if_present(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("removing {}: {error}", path.display()),
    }
}

/// <path>'s metadata, following links. None when the path does not exist.
pub(crate) fn stat(path: &Path) -> Result<Option<std::fs::Metadata>> {
    metadata_of(path, std::fs::metadata(path))
}

/// <path>'s own metadata, including a link to a missing target.
pub(crate) fn lstat(path: &Path) -> Result<Option<std::fs::Metadata>> {
    metadata_of(path, std::fs::symlink_metadata(path))
}

fn metadata_of(
    path: &Path,
    read: std::io::Result<std::fs::Metadata>,
) -> Result<Option<std::fs::Metadata>> {
    match read {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if fail::gone(&error) => Ok(None),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}
