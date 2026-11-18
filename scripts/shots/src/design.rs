//! The design files the boards render from: the design system the boards
//! load, a board's frame, and the hash of everything a board's picture
//! depends on.

use crate::render::preview::{REACT, REACT_DOM};
use boards::{MOCKS, Source, board_path};
use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The design system's built files, as the hashes name them: their paths in
/// the repo.
pub(crate) const TOKENS: &str = boards::TOKENS_CSS;
pub(crate) const BUNDLE_CSS: &str = "design/system/components/bundle.css";
pub(crate) const BUNDLE_JS: &str = "design/system/components/bundle.js";

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
    #[error("board {board} imports {part}, but {file} does not exist")]
    MissingPart {
        board: String,
        part: String,
        file: PathBuf,
    },
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

    /// The built files under the names the hashes give them.
    fn built(&self) -> [(&'static str, &str); 3] {
        [
            (TOKENS, &self.tokens_css),
            (BUNDLE_CSS, &self.bundle_css),
            (BUNDLE_JS, &self.bundle_js),
        ]
    }
}

fn read(path: &Path) -> Result<Vec<u8>, DesignError> {
    std::fs::read(path).map_err(|e| DesignError::Unreadable {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

/// The file of the board `name`.
pub(crate) fn board_file(root: &Path, name: &str) -> PathBuf {
    root.join(board_path(name))
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

/// The board's file and every part it mounts with `dc-import`, parts of parts
/// included, by name.
fn reached(root: &Path, name: &str) -> Result<BTreeMap<String, PathBuf>, DesignError> {
    let import = Regex::new(r#"<dc-import\s[^>]*?\bname="([^"]+)""#).unwrap();
    let mut files = BTreeMap::new();
    let mut pending = vec![(name.to_string(), name.to_string())];
    let mut seen = BTreeSet::new();
    while let Some((part, importer)) = pending.pop() {
        if !seen.insert(part.clone()) {
            continue;
        }
        let file = board_file(root, &part);
        if !file.is_file() {
            return Err(DesignError::MissingPart {
                board: importer,
                part,
                file,
            });
        }
        let html = String::from_utf8_lossy(&read(&file)?).into_owned();
        for captures in import.captures_iter(&html) {
            pending.push((captures[1].to_string(), part.clone()));
        }
        files.insert(part, file);
    }
    Ok(files)
}

fn feed(hasher: &mut Sha256, label: &str, bytes: &[u8]) {
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// What a board's picture depends on: its file and the parts it mounts, the
/// design system's built token sheet, style sheet and script, the runtime
/// copy and the browser.
pub(crate) fn board_hash(
    root: &Path,
    name: &str,
    design: &DesignSystem,
    runtime_sha256: &str,
    browser_version: &str,
) -> Result<String, DesignError> {
    let mut hasher = Sha256::new();
    for (part, file) in reached(root, name)? {
        feed(&mut hasher, &format!("board {part}"), &read(&file)?);
    }
    for (label, text) in design.built() {
        feed(&mut hasher, label, text.as_bytes());
    }
    feed(&mut hasher, "runtime", runtime_sha256.as_bytes());
    feed(&mut hasher, "browser", browser_version.as_bytes());
    Ok(crate::hex::hex(&hasher.finalize()))
}

/// What a component state's picture depends on: the component's preview page,
/// the design system's built token sheet, style sheet and script, the copies
/// of React and the browser.
pub(crate) fn preview_hash(
    root: &Path,
    component: &str,
    design: &DesignSystem,
    browser_version: &str,
) -> Result<String, DesignError> {
    let mut hasher = Sha256::new();
    let preview = root.join(boards::preview::preview_path(component));
    feed(
        &mut hasher,
        &format!("preview {component}"),
        &read(&preview)?,
    );
    for (label, text) in design.built() {
        feed(&mut hasher, label, text.as_bytes());
    }
    feed(&mut hasher, "react", REACT.as_bytes());
    feed(&mut hasher, "react-dom", REACT_DOM.as_bytes());
    feed(&mut hasher, "browser", browser_version.as_bytes());
    Ok(crate::hex::hex(&hasher.finalize()))
}

#[cfg(test)]
#[path = "design_tests.rs"]
pub(crate) mod tests;
