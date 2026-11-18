//! How a repo's boards split among the claude.ai canvases they publish to. A
//! canvas holds at most `MAX_FILES` files, so a repo with more boards
//! publishes them to several canvases, split by page:
//! `design/mocks/canvases.toml` names each canvas's link, title, launch page
//! and pages, and the link of the Design System artifact that the repo's
//! design system publishes to beside them. Each canvas shows its boards, its
//! part of `canvas.json`, and a copy of the design system under
//! `project/ds/<namespace>/` (`design_publish` writes the files).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use boards::{BOARD_SUFFIX, CANVAS, imports};

use crate::design_system_artifact::SHARED_FILES;
use crate::fail;
use crate::fail::{Fail, Result};

/// The file that names the artifacts the repo's designs publish to: each
/// canvas with the pages it shows, and the design system.
pub const CANVASES: &str = "design/mocks/canvases.toml";
/// Where a canvas keeps its index, the part of the repo's `CANVAS` it shows.
pub const PUBLISHED_INDEX: &str = "project/canvas.json";
/// How every artifact link starts; the artifact's id follows.
const LINK: &str = "https://claude.ai/artifact/";
/// The most files a canvas holds.
pub const MAX_FILES: usize = 512;
/// Where a canvas keeps its copy of the design system's shared files.
const DS_COPY: &str = "project/ds/";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanvasesFile {
    design_system: Option<DesignSystemEntry>,
    #[serde(default)]
    canvas: Vec<Canvas>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesignSystemEntry {
    url: String,
}

/// One canvas of `CANVASES`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    /// The canvas's link, `https://claude.ai/artifact/<id>`.
    pub url: String,
    /// The title its index gives it.
    pub title: String,
    /// The page it opens on.
    pub launch: String,
    /// The ids of the pages of `canvas.json` it publishes.
    pub pages: Vec<String>,
}

/// The path on a canvas of its copy of <rel>, one of the design system's
/// `SHARED_FILES`, for the design system whose namespace is <namespace>.
pub fn ds_copy(namespace: &str, rel: &str) -> String {
    format!("{DS_COPY}{namespace}/{rel}")
}

/// Whether the repo decides a canvas's file at <path>, so that a path the
/// commit lacks leaves the canvas: every file under `project/`.
pub fn repo_decides(path: &str) -> bool {
    path.starts_with("project/")
}

/// Where <path> goes among a canvas's calls, lower first: its index, then
/// its copy of the design system, then the boards.
pub fn rank(path: &str) -> u8 {
    if path == PUBLISHED_INDEX {
        0
    } else if path.starts_with(DS_COPY) {
        1
    } else {
        2
    }
}

/// What `CANVASES` names: the canvases, and the link of the design system's
/// artifact.
#[derive(Debug)]
pub struct Published {
    pub canvases: Vec<Canvas>,
    pub design_system: String,
}

/// The id of the artifact that <url> links to, the end of its link; None
/// when <url> is no artifact link.
pub fn artifact_id(url: &str) -> Option<&str> {
    url.strip_prefix(LINK)
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// What <text>, a `CANVASES` file that <label> names in an error, names.
pub fn parse_canvases(text: &str, label: &str) -> Result<Published> {
    let file: CanvasesFile =
        toml::from_str(text).map_err(|error| Fail::Message(format!("{label}: {error}")))?;
    if file.canvas.is_empty() {
        fail!(
            "{label} names no canvas: give each as a [[canvas]] table with its url, title, launch and pages"
        );
    }
    let Some(design_system) = file.design_system else {
        fail!(
            "{label} names no design system: give its artifact's link as url in a [design_system] table, so that every publish sends the design system with the canvases"
        );
    };
    let mut links: Vec<&str> = Vec::new();
    for url in file
        .canvas
        .iter()
        .map(|canvas| canvas.url.as_str())
        .chain([design_system.url.as_str()])
    {
        if artifact_id(url).is_none() {
            fail!("{label}: {url:?} is no artifact link; an artifact link is {LINK}<id>");
        }
        if links.contains(&url) {
            fail!("{label} names the artifact {url} twice");
        }
        links.push(url);
    }
    Ok(Published {
        canvases: file.canvas,
        design_system: design_system.url,
    })
}

/// What one canvas publishes: its index and the names of its boards.
#[derive(Debug)]
pub struct Split<'a> {
    pub canvas: &'a Canvas,
    pub index: Value,
    pub boards: Vec<String>,
}

/// The page that <entry>, the <kind> <name> of the index, is on: its own, or
/// the first page when it names none, as the canvas places it.
fn page_of<'a>(kind: &str, name: &str, entry: &'a Value, first: &'a str) -> Result<&'a str> {
    match entry.get("page") {
        None => Ok(first),
        Some(Value::String(page)) => Ok(page),
        Some(other) => fail!("{CANVAS}: {kind} {name} gives the page {other}, which is no page id"),
    }
}

/// Splits <index>, the repo's `canvas.json`, and the board files <boards>
/// (name to text) among <canvases> by page. Fails when a page is on no
/// canvas or on two, a canvas names a page the index does not list or opens
/// on a page it does not publish, the index and the board files name
/// different boards, a board mounts a board of another canvas, or a canvas
/// would hold more than `MAX_FILES` files.
pub fn split<'a>(
    index: &Value,
    canvases: &'a [Canvas],
    boards: &BTreeMap<String, String>,
) -> Result<Vec<Split<'a>>> {
    let Some(page_list) = index["pages"].as_array() else {
        fail!("{CANVAS} lists no pages, and the canvases are split by page");
    };
    let mut pages: Vec<&str> = Vec::new();
    for (n, page) in page_list.iter().enumerate() {
        let Some(id) = page["id"].as_str() else {
            fail!("{CANVAS}: page {} of `pages` ({page}) has no id", n + 1);
        };
        pages.push(id);
    }
    let Some(first) = pages.first().copied() else {
        fail!("{CANVAS} lists no pages, and the canvases are split by page");
    };
    let mut owner: BTreeMap<&str, usize> = BTreeMap::new();
    for (n, canvas) in canvases.iter().enumerate() {
        for page in &canvas.pages {
            if !pages.contains(&page.as_str()) {
                fail!(
                    "{CANVASES}: {} lists page {page}, which {CANVAS} does not list",
                    canvas.url
                );
            }
            if let Some(other) = owner.insert(page, n) {
                fail!(
                    "{CANVASES}: page {page} is on two canvases, {} and {}",
                    canvases[other].url,
                    canvas.url
                );
            }
        }
        if !canvas.pages.contains(&canvas.launch) {
            fail!(
                "{CANVASES}: {} launches on page {}, which is not one of its pages",
                canvas.url,
                canvas.launch
            );
        }
    }
    if let Some(page) = pages.iter().find(|page| !owner.contains_key(*page)) {
        fail!("{CANVASES}: page {page} is on no canvas; add it to the pages of one");
    }
    let Some(placed) = index["boards"].as_object() else {
        fail!("{CANVAS}: `boards` is not an object of board files");
    };
    if let Some(name) = boards.keys().find(|name| !placed.contains_key(*name)) {
        fail!("{name} is a board file that {CANVAS} does not place");
    }
    if let Some(name) = placed.keys().find(|name| !boards.contains_key(*name)) {
        fail!("{CANVAS} places {name}, which is no board file");
    }
    let no_notes = Map::new();
    let notes = match index.get("notes") {
        None => &no_notes,
        Some(Value::Object(notes)) => notes,
        Some(_) => fail!("{CANVAS}: `notes` is not an object of notes"),
    };
    // The canvas that each board and each note is on, by its page.
    let place = |kind: &str, entries: &Map<String, Value>| -> Result<BTreeMap<String, usize>> {
        let mut on = BTreeMap::new();
        for (name, entry) in entries {
            let page = page_of(kind, name, entry, first)?;
            let Some(canvas) = owner.get(page) else {
                fail!("{CANVAS}: {kind} {name} is on page {page}, which {CANVAS} does not list");
            };
            on.insert(name.clone(), *canvas);
        }
        Ok(on)
    };
    let board_canvas = place("board", placed)?;
    let note_canvas = place("note", notes)?;
    for (name, text) in boards {
        let on = board_canvas[name];
        for mounted in imports(text) {
            let file = format!("{mounted}{BOARD_SUFFIX}");
            let Some(&other) = board_canvas.get(&file) else {
                fail!("{name} mounts {file}, which is no board file");
            };
            if other != on {
                fail!(
                    "{name} on {} mounts {file}, which is on {}; a board mounts only boards of its own canvas",
                    canvases[on].url,
                    canvases[other].url
                );
            }
        }
    }
    let mut splits = Vec::new();
    for (n, canvas) in canvases.iter().enumerate() {
        let mine: Map<String, Value> = placed
            .iter()
            .filter(|(name, _)| board_canvas[*name] == n)
            .map(|(name, entry)| (name.clone(), entry.clone()))
            .collect();
        let files = mine.len() + 1 + SHARED_FILES.len();
        if files > MAX_FILES {
            fail!(
                "{} would hold {files} files, and a canvas holds at most {MAX_FILES}; move a page to another canvas in {CANVASES}",
                canvas.url
            );
        }
        let my_notes: Map<String, Value> = notes
            .iter()
            .filter(|(id, _)| note_canvas[*id] == n)
            .map(|(id, note)| (id.clone(), note.clone()))
            .collect();
        let mut part = index.clone();
        part["title"] = json!(canvas.title);
        part["launch"] = json!({"view": "canvas", "page": canvas.launch});
        part["pages"] = Value::Array(
            page_list
                .iter()
                .zip(&pages)
                .filter(|(_, id)| owner.get(**id) == Some(&n))
                .map(|(page, _)| page.clone())
                .collect(),
        );
        for (key, kept) in [("order", &mine), ("noteOrder", &my_notes)] {
            match index.get(key) {
                None => {}
                Some(Value::Array(order)) => {
                    part[key] = Value::Array(
                        order
                            .iter()
                            .filter(|entry| entry.as_str().is_some_and(|e| kept.contains_key(e)))
                            .cloned()
                            .collect(),
                    );
                }
                Some(_) => fail!("{CANVAS}: `{key}` is not a list"),
            }
        }
        if index.get("notes").is_some() {
            part["notes"] = Value::Object(my_notes);
        }
        let names = mine.keys().cloned().collect();
        part["boards"] = Value::Object(mine);
        splits.push(Split {
            canvas,
            index: part,
            boards: names,
        });
    }
    Ok(splits)
}

#[cfg(test)]
#[path = "design_canvas_tests.rs"]
pub(crate) mod tests;
