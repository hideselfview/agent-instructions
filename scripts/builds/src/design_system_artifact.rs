//! The files of the claude.ai Design System artifact that a repo's design
//! system publishes to, in the shape that artifact type keeps them under
//! `project/`: the brand book (`README.md`), `tokens.json` and the
//! `tokens.css` builds writes from it, and `components/` with the bundles
//! the commit's build-bundle.py builds, and `assets/` (`design_system_assets`).
//! The artifact's index (`project/design-system.json`) is the page's own
//! record, which only the live artifact holds: a session reads it, and builds
//! sets its asset records and its `lastChange`, which says which commit the
//! files come from.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::design_system_assets::{self as assets, Upload};
use crate::fail;
use crate::fail::{Context, Result};

/// The design system's folder in the repo.
pub const SYSTEM: &str = "design/system";
/// The artifact's index, which the session reads from the live artifact.
pub const INDEX: &str = "project/design-system.json";
/// The most files the artifact type keeps of an artifact's own.
pub const MAX_FILES: usize = 4080;
/// The design system's files that every preview loads, under `project/`:
/// the tokens, and the stylesheet and bundles builds generates. Each canvas
/// carries a copy of them too, under `project/ds/<namespace>/`.
pub const SHARED_FILES: [&str; 4] = [
    "tokens.json",
    "tokens.css",
    "components/bundle.css",
    "components/bundle.js",
];
/// The component the artifact type shows above the brand book, which it asks
/// to be written last.
const COVER: &str = "project/components/Cover/";
/// The file that a call which only removes files names as its `file_path`,
/// which every publish needs: one the artifact publishes anyway.
pub const REMOVAL_ANCHOR: &str = "project/README.md";

/// What the artifact publishes from one commit.
#[derive(Debug)]
pub struct Contents {
    /// The files, by their paths on the artifact.
    pub files: BTreeMap<String, String>,
    /// The uploads, each with its bytes, in the order of their paths.
    pub uploads: Vec<Upload>,
}

/// What the artifact publishes from <export>, a folder that holds the
/// repo's `SYSTEM` at its path with its links resolved, and <generated>, the
/// tokens.css, bundle.css and bundle.js that `Source::design_system` gives:
/// `SYSTEM`'s README.md and tokens.json, every file under its
/// `components/`, the generated files at their places in `SHARED_FILES`,
/// and its `assets/`. Fails when a component's folder name is no JavaScript
/// identifier, which the artifact type refuses, or the artifact would hold
/// more than `MAX_FILES` files.
pub fn contents(export: &Path, generated: &[String; 3]) -> Result<Contents> {
    let system = export.join(SYSTEM);
    let mut files = BTreeMap::new();
    for name in ["README.md", "tokens.json"] {
        let from = system.join(name);
        let text = std::fs::read_to_string(&from).context(|| format!("reading {SYSTEM}/{name}"))?;
        files.insert(format!("project/{name}"), text);
    }
    let components = system.join("components");
    for entry in
        std::fs::read_dir(&components).context(|| format!("listing {SYSTEM}/components"))?
    {
        let entry = entry.context(|| format!("listing {SYSTEM}/components"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() && !is_identifier(&name) {
            fail!(
                "{SYSTEM}/components/{name} is a component folder whose name is no JavaScript identifier, which the Design System artifact refuses"
            );
        }
    }
    add_tree(&components, "project/components", &mut files)?;
    for (rel, text) in SHARED_FILES[1..].iter().zip(generated) {
        files.insert(format!("project/{rel}"), text.clone());
    }
    let (texts, uploads) = assets::read(export)?;
    files.extend(texts);
    if files.len() > MAX_FILES {
        fail!(
            "the design system would publish {} files, and a Design System artifact holds at most {MAX_FILES}",
            files.len()
        );
    }
    Ok(Contents { files, uploads })
}

/// Adds every file under <dir> to <files> at <prefix>/<its path under dir>.
fn add_tree(dir: &Path, prefix: &str, files: &mut BTreeMap<String, String>) -> Result<()> {
    for rel in files_under(dir)? {
        let path = dir.join(&rel);
        let text =
            std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
        files.insert(format!("{prefix}/{rel}"), text);
    }
    Ok(())
}

/// The path of every file under <dir>, relative to it, following links.
pub fn files_under(dir: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut todo = vec![String::new()];
    while let Some(rel) = todo.pop() {
        let at = dir.join(&rel);
        for entry in std::fs::read_dir(&at).context(|| format!("listing {}", at.display()))? {
            let entry = entry.context(|| format!("listing {}", at.display()))?;
            let name = format!("{rel}{}", entry.file_name().to_string_lossy());
            if entry.path().is_dir() {
                todo.push(format!("{name}/"));
            } else {
                found.push(name);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Whether <name> is an ASCII JavaScript identifier.
fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'$')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
}

/// Where <path> goes among the artifact's calls, lower first: the shared
/// files first, since every preview loads them, and the cover last, as the
/// artifact type asks.
pub fn rank(path: &str) -> u8 {
    if SHARED_FILES
        .iter()
        .any(|rel| path.strip_prefix("project/") == Some(rel))
    {
        0
    } else if path.starts_with(COVER) {
        2
    } else {
        1
    }
}

/// Whether the repo's design system decides the file at <path> of the
/// artifact, so that a path the commit lacks leaves the artifact: every path
/// under `project/` but the index, the files the page generates (`api/`
/// cards and `manifest.json`) and the page's own state files, whose names
/// start with a dot. The index's records name the uploads, which no listing
/// of the artifact's files shows.
pub fn repo_decides(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("project/") else {
        return false;
    };
    !(path == INDEX
        || rest == "manifest.json"
        || rest.starts_with("api/")
        || rest.split('/').any(|part| part.starts_with('.')))
}

/// The live artifact's index from <text>. Fails when the text is no index:
/// a JSON object without the marker (`createdOnFiles` or `convertedFrom`)
/// that the artifact type writes, which the type says never to write over.
pub fn parse_index(text: &str) -> Result<Map<String, Value>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| crate::fail::Fail::Message(format!("the index is no JSON: {error}")))?;
    let Value::Object(index) = value else {
        fail!("the index is no JSON object");
    };
    if !index.contains_key("createdOnFiles") && !index.contains_key("convertedFrom") {
        fail!(
            "the index has no createdOnFiles or convertedFrom marker, so it is no Design System index; publish nothing over it"
        );
    }
    Ok(index)
}

/// Sets <index>'s `lastChange` to say that <by> published the files of
/// commit <commit> at <at> (ISO 8601).
pub fn set_last_change(index: &mut Map<String, Value>, by: &str, at: &str, commit: &str) {
    index.insert(
        "lastChange".to_string(),
        json!({
            "by": by,
            "at": at,
            "via": "Claude Code",
            "note": format!("Published from commit {commit} of the repo's {SYSTEM}."),
        }),
    );
}

/// <index> as the text of the index file.
pub fn index_text(index: &Map<String, Value>) -> String {
    serde_json::to_string_pretty(index).expect("the index serializes") + "\n"
}

#[cfg(test)]
#[path = "design_system_artifact_tests.rs"]
mod tests;
