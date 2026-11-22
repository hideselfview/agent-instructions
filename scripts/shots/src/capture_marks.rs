use std::io;
use std::path::{Path, PathBuf};

use crate::names::{NameError, Prefix};

/// The folder in an app tree that marks the prefixes the checker has captured.
pub const CAPTURE_MARKS_DIR: &str = ".taken";

#[derive(Debug, thiserror::Error)]
pub enum CaptureMarkError {
    #[error("cannot {action} {}: {source}", path.display())]
    Storage {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("expected a capture mark file at {}", .0.display())]
    NotFile(PathBuf),
    #[error("expected a capture mark named <prefix>@windows or <prefix>@no-windows at {}", .0.display())]
    Name(PathBuf),
    #[error("{} holds an invalid prefix: {source}", path.display())]
    Prefix {
        path: PathBuf,
        #[source]
        source: NameError,
    },
}

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
    pub fn write(&self, prefixes: &[Prefix], windows: bool) -> Result<(), CaptureMarkError> {
        storage("prepare", &self.dir, std::fs::create_dir_all(&self.dir))?;
        let mark = if windows { "windows" } else { "no-windows" };
        for prefix in prefixes {
            let file = self.dir.join(format!("{}@{mark}", prefix.as_str()));
            match std::fs::File::create_new(&file) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let metadata = storage(
                        "read the file type of",
                        &file,
                        std::fs::symlink_metadata(&file),
                    )?;
                    require_file(metadata.file_type(), &file)?;
                }
                Err(source) => {
                    return Err(CaptureMarkError::Storage {
                        action: "write",
                        path: file,
                        source,
                    });
                }
            }
        }
        Ok(())
    }

    /// Whether the tree keeps every requested prefix and the window shots it asks for.
    pub fn cover(&self, prefixes: &[&str], windows: bool) -> Result<bool, CaptureMarkError> {
        let contents = storage("list", &self.dir, std::fs::read_dir(&self.dir))?;
        let mut covered = vec![false; prefixes.len()];
        for entry in contents {
            let entry = storage("list", &self.dir, entry)?;
            let path = entry.path();
            let kind = storage("read the file type of", &path, entry.file_type())?;
            require_file(kind, &path)?;
            let name = entry.file_name();
            let mark = name.to_str().and_then(|name| name.rsplit_once('@'));
            let (marked, took_windows) = match mark {
                Some((marked, "windows")) => (marked, true),
                Some((marked, "no-windows")) => (marked, false),
                _ => return Err(CaptureMarkError::Name(path)),
            };
            let marked = Prefix::parse(marked)
                .map_err(|source| CaptureMarkError::Prefix { path, source })?;
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

fn require_file(kind: std::fs::FileType, path: &Path) -> Result<(), CaptureMarkError> {
    if kind.is_file() {
        Ok(())
    } else {
        Err(CaptureMarkError::NotFile(path.to_path_buf()))
    }
}

fn storage<T>(
    action: &'static str,
    path: &Path,
    result: io::Result<T>,
) -> Result<T, CaptureMarkError> {
    result.map_err(|source| CaptureMarkError::Storage {
        action,
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[path = "capture_marks_tests.rs"]
mod tests;
