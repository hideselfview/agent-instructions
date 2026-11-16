//! Writing a file whole: a reader of the file sees its old contents or its
//! new ones, never part of them.

use std::path::Path;

use crate::fail::{Context, Result};

/// Writes <contents> to `<file>.part`, then renames that onto <file> in one
/// step. Two writers of one file must not run at once: they share the part.
pub fn write(file: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let part = file.with_extension("part");
    std::fs::write(&part, contents).context(|| format!("writing {}", part.display()))?;
    std::fs::rename(&part, file).context(|| format!("replacing {}", file.display()))
}
