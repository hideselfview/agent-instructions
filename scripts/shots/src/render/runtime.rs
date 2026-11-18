//! The canvas runtime copy the boards are drawn with, and the check that it is
//! the runtime the canvas serves.
//!
//! Each copy is compiled into builds, and the boards crate's test checks that
//! each one hashes to its sha256, so a run trusts the copies' sha256s.
//!
//! builds keeps the copies by sha256 (`boards::runtime`). A repo that publishes
//! a canvas records the runtime it was published with in
//! `design/mocks/runtime.toml`, which the design session writes at each canvas
//! publish; only a session with the person's login can read the canvas, so
//! this program trusts that record. Its boards are drawn with the copy of
//! that sha256, and drawing fails when builds has no such copy. A repo with no
//! record publishes no canvas, and its boards are drawn with the newest copy.

use boards::runtime::{self as copies, Record as LiveRecord, RuntimeCopy};
use std::path::{Path, PathBuf};

pub(crate) use boards::runtime::RECORD as LIVE_RECORD;

/// How to add a copy of the runtime the canvas at `canvas` serves.
fn refresh(canvas: &str) -> String {
    format!(
        "To add the copy: read artifact-type/dc-runtime.js of {canvas} with the Artifact tool \
(action read), save it in agent-instructions as scripts/boards/runtime/<its sha256>.js, add its \
entry to COPIES in scripts/boards/src/runtime.rs, and install builds."
    )
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub(crate) enum RuntimeError {
    #[error("cannot read {path}: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    #[error("{path} is not a valid record: {reason}")]
    BadRecord { path: PathBuf, reason: String },
    #[error(
        "builds has no copy of the runtime {} ({} of canvas type release {}) that the canvas was \
published with at {} by {}. {}",
        .0.runtime_sha256, .0.runtime, .0.type_release, .0.published_at, .0.published_by,
        refresh(&.0.canvas)
    )]
    /// The live record names a runtime builds has no copy of.
    NoCopy(Box<LiveRecord>),
}

/// The copy the boards are drawn with.
#[derive(Debug, PartialEq)]
pub(crate) struct Checked {
    pub(crate) copy: &'static RuntimeCopy,
    /// What the report says of the runtime: the copy's version and when the
    /// canvas was published with it, or that the repo publishes no canvas.
    pub(crate) said: String,
}

/// The copy the boards of the repo at `root` are drawn with.
pub(crate) fn check(root: &Path) -> Result<Checked, RuntimeError> {
    let path = root.join(LIVE_RECORD);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let copy = copies::newest();
            return Ok(Checked {
                copy,
                said: format!(
                    "the newest copy, version {} (builds read it {}): the repo has no \
{LIVE_RECORD}",
                    copy.version, copy.read_at
                ),
            });
        }
        Err(e) => {
            return Err(RuntimeError::Unreadable {
                path,
                reason: e.to_string(),
            });
        }
    };
    let live: LiveRecord = toml::from_str(&text).map_err(|e| RuntimeError::BadRecord {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    let Some(copy) = copies::copy(&live.runtime_sha256) else {
        return Err(RuntimeError::NoCopy(Box::new(live)));
    };
    Ok(Checked {
        copy,
        said: format!(
            "version {}, canvas {} published {}",
            copy.version, live.canvas_version, live.published_at
        ),
    })
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
