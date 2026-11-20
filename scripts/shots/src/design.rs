//! The design files the boards render from: the design system the boards
//! load and a board's frame.

use boards::{MOCKS, Source};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub(crate) enum DesignError {
    #[error("{0}")]
    Build(#[from] boards::Error),
    #[error("cannot read {path}: {reason}")]
    Unreadable { path: PathBuf, reason: String },
    #[error("{0} is not valid canvas.json: {1}")]
    BadCanvas(PathBuf, String),
    #[error("canvas.json has no board {0}")]
    NoFrame(String),
}

/// A board's frame on the canvas, in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct Frame {
    pub(crate) w: u32,
    pub(crate) h: u32,
}

#[derive(Deserialize)]
struct Canvas {
    boards: BTreeMap<String, Frame>,
}

/// The design system the boards load, built from the repo's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesignSystem {
    /// The folder the boards load it from, `ds/<namespace>/`.
    pub(crate) namespace: String,
    pub(crate) tokens_css: String,
    pub(crate) bundle_css: String,
    pub(crate) bundle_js: String,
}

impl DesignSystem {
    /// The design system of the repo at <root>, its bundles built in a
    /// scratch folder in <cache>; nothing is written into the repo.
    pub(crate) fn of(root: &Path, cache: &Path) -> Result<Self, DesignError> {
        let canvas = root.join(boards::CANVAS);
        let index: serde_json::Value = serde_json::from_slice(&read(&canvas)?)
            .map_err(|e| DesignError::BadCanvas(canvas, e.to_string()))?;
        let namespace = boards::design_namespace(&index)?;
        let [tokens_css, bundle_css, bundle_js] = Source::Dir(root).design_system(cache)?;
        Ok(Self {
            namespace,
            tokens_css,
            bundle_css,
            bundle_js,
        })
    }
}

fn read(path: &Path) -> Result<Vec<u8>, DesignError> {
    std::fs::read(path).map_err(|e| DesignError::Unreadable {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

/// The boards the design session marks ready to be shot, one name a line.
pub(crate) fn ready_boards(root: &Path) -> Result<Vec<String>, DesignError> {
    let text = String::from_utf8_lossy(&read(&root.join(MOCKS).join("shots.txt"))?).into_owned();
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect())
}

/// The board's frame from `canvas.json`.
pub(crate) fn frame(root: &Path, name: &str) -> Result<Frame, DesignError> {
    let path = root.join(boards::CANVAS);
    let canvas: Canvas = serde_json::from_slice(&read(&path)?)
        .map_err(|e| DesignError::BadCanvas(path, e.to_string()))?;
    canvas
        .boards
        .get(&format!("{name}{}", boards::BOARD_SUFFIX))
        .copied()
        .ok_or_else(|| DesignError::NoFrame(name.to_string()))
}

#[cfg(test)]
#[path = "design_tests.rs"]
pub(crate) mod tests;
