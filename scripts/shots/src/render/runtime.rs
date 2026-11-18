//! The canvas runtime copy the boards are drawn with, and the check that it is
//! the runtime the canvas serves.
//!
//! builds keeps the copies by sha256 (`boards::runtime`). A repo that publishes
//! a canvas records the runtime it was published with in
//! `design/mocks/runtime.toml`, which the design session writes at each canvas
//! publish; only a session with the person's login can read the canvas, so
//! this program trusts that record. Its boards are drawn with the copy of
//! that sha256, and drawing fails when builds has no such copy. A repo with no
//! record publishes no canvas, and its boards are drawn with the newest copy.

use boards::runtime::{self as copies, RuntimeCopy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) const LIVE_RECORD: &str = "design/mocks/runtime.toml";

/// How to add a copy of the runtime the canvas at `canvas` serves.
fn refresh(canvas: &str) -> String {
    format!(
        "To add the copy: read artifact-type/dc-runtime.js of {canvas} with the Artifact tool \
(action read), save it in agent-instructions as scripts/boards/runtime/<its sha256>.js, add its \
entry to COPIES in scripts/boards/src/runtime.rs, and install builds."
    )
}

/// `design/mocks/runtime.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveRecord {
    canvas: String,
    canvas_version: String,
    /// The canvas type's release the runtime was read from.
    type_release: String,
    /// Where the runtime sits in the canvas type.
    runtime: String,
    runtime_sha256: String,
    published_at: String,
    published_by: String,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub(crate) enum RuntimeError {
    #[error("cannot read {path}: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    #[error("{path} is not a valid record: {reason}")]
    BadRecord { path: PathBuf, reason: String },
    #[error(
        "the copy {file} hashes to {actual}, not the sha256 its name and entry give; the copy was \
edited. {}",
        refresh(.canvas)
    )]
    CopyChanged {
        file: String,
        actual: String,
        canvas: String,
    },
    #[error(
        "builds has no copy of the runtime {} ({} of canvas type release {}) that the canvas was \
published with at {} by {}. {}",
        .0.runtime_sha256, .0.runtime, .0.type_release, .0.published_at, .0.published_by,
        refresh(&.0.canvas)
    )]
    NoCopy(Box<NoCopy>),
}

/// A live record that names a runtime builds has no copy of.
#[derive(Debug, PartialEq)]
pub(crate) struct NoCopy {
    runtime_sha256: String,
    runtime: String,
    type_release: String,
    published_at: String,
    published_by: String,
    canvas: String,
}

/// The copy the boards are drawn with.
#[derive(Debug, PartialEq)]
pub(crate) struct Checked {
    pub(crate) copy: &'static RuntimeCopy,
    /// What the report says of the runtime: the copy's version and when the
    /// canvas was published with it, or that the repo publishes no canvas.
    pub(crate) said: String,
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Checks that a copy hashes to its sha256.
fn whole(copy: &'static RuntimeCopy, canvas: &str) -> Result<(), RuntimeError> {
    let actual = sha256(copy.text.as_bytes());
    if actual == copy.sha256 {
        return Ok(());
    }
    Err(RuntimeError::CopyChanged {
        file: copies::copy_file(copy.sha256),
        actual,
        canvas: canvas.to_string(),
    })
}

/// The copy the boards of the repo at `root` are drawn with.
pub(crate) fn check(root: &Path) -> Result<Checked, RuntimeError> {
    let path = root.join(LIVE_RECORD);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let copy = copies::newest();
            whole(copy, copy.source)?;
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
        return Err(RuntimeError::NoCopy(Box::new(NoCopy {
            runtime_sha256: live.runtime_sha256,
            runtime: live.runtime,
            type_release: live.type_release,
            published_at: live.published_at,
            published_by: live.published_by,
            canvas: live.canvas,
        })));
    };
    whole(copy, &live.canvas)?;
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
