use crate::names::Prefix;
use std::path::{Path, PathBuf};

/// The folder in an app tree that marks the prefixes the checker has captured.
pub const CAPTURE_MARKS_DIR: &str = ".taken";

/// The capture marks for one app tree.
pub struct CaptureMarks {
    dir: PathBuf,
}

impl CaptureMarks {
    pub fn of(tree: &Path) -> Self {
        Self {
            dir: tree.join(CAPTURE_MARKS_DIR),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.dir
    }

    /// Mark each prefix after a run captures every shot that it selects.
    pub fn write(&self, prefixes: &[String], windows: bool) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("cannot prepare {}: {error}", self.dir.display()))?;
        let mark = if windows { "windows" } else { "no-windows" };
        for prefix in prefixes {
            let prefix = Prefix::parse(prefix).map_err(|error| error.to_string())?;
            let file = self.dir.join(format!("{}@{mark}", prefix.as_str()));
            match std::fs::File::create_new(&file) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let kind = std::fs::symlink_metadata(&file)
                        .map_err(|error| {
                            format!("cannot read the file type of {}: {error}", file.display())
                        })?
                        .file_type();
                    require_file(kind, &file)?;
                }
                Err(error) => return Err(format!("cannot write {}: {error}", file.display())),
            }
        }
        Ok(())
    }

    /// Whether the tree keeps every requested prefix and the window shots it asks for.
    pub fn cover(&self, prefixes: &[&str], windows: bool) -> Result<bool, String> {
        let contents = std::fs::read_dir(&self.dir)
            .map_err(|error| format!("cannot list {}: {error}", self.dir.display()))?;
        let mut covered = vec![false; prefixes.len()];
        for entry in contents {
            let entry =
                entry.map_err(|error| format!("cannot list {}: {error}", self.dir.display()))?;
            let kind = entry.file_type().map_err(|error| {
                format!(
                    "cannot read the file type of {}: {error}",
                    entry.path().display()
                )
            })?;
            require_file(kind, &entry.path())?;
            let name = entry.file_name();
            let mark = name.to_str().and_then(|name| name.rsplit_once('@'));
            let (marked, took_windows) = match mark {
                Some((marked, "windows")) => (marked, true),
                Some((marked, "no-windows")) => (marked, false),
                _ => {
                    return Err(format!(
                        "{} is no mark the shots program writes (<prefix>@windows or <prefix>@no-windows)",
                        entry.path().display()
                    ));
                }
            };
            let marked = Prefix::parse(marked).map_err(|error| {
                format!(
                    "{} holds an invalid prefix: {error}",
                    entry.path().display()
                )
            })?;
            if took_windows || !windows {
                for (prefix, has_mark) in prefixes.iter().zip(&mut covered) {
                    if !*has_mark && prefix.starts_with(marked.as_str()) {
                        *has_mark = true;
                    }
                }
            }
        }
        Ok(covered.into_iter().all(|has_mark| has_mark))
    }
}

fn require_file(kind: std::fs::FileType, path: &Path) -> Result<(), String> {
    if kind.is_file() {
        Ok(())
    } else {
        Err(format!(
            "expected a capture mark file at {}",
            path.display()
        ))
    }
}

#[cfg(test)]
#[path = "capture_marks_tests.rs"]
mod tests;
