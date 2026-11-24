//! How a repo's boards split among the claude.ai canvases they publish to. A
//! canvas holds at most `MAX_FILES` files, so a repo with more boards
//! publishes them to several canvases, split by page:
//! `design/mocks/canvases.toml` names each canvas's title, launch page and
//! pages, and the title of the Design System artifact that the repo's design
//! system publishes to beside them (`design_artifacts` finds each title on
//! the account). Each canvas shows its boards, its part of `canvas.json`, and
//! a copy of the design system under `project/ds/<namespace>/`
//! (`design_publish` writes the files).

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
    title: String,
}

/// One canvas of `CANVASES`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    /// The title its index gives it, by which the publish finds it on the
    /// account.
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

/// What `CANVASES` names: the canvases, and the title of the design system's
/// artifact.
#[derive(Debug)]
pub struct Published {
    pub canvases: Vec<Canvas>,
    pub design_system: String,
}

/// What <text>, a `CANVASES` file that <label> names in an error, names.
pub fn parse_canvases(text: &str, label: &str) -> Result<Published> {
    let file: CanvasesFile =
        toml::from_str(text).map_err(|error| Fail::Message(format!("{label}: {error}")))?;
    if file.canvas.is_empty() {
        fail!(
            "{label} names no canvas: give each as a [[canvas]] table with its title, launch and pages"
        );
    }
    let Some(design_system) = file.design_system else {
        fail!(
            "{label} names no design system: give its artifact's title as title in a [design_system] table, so that every publish sends the design system with the canvases"
        );
    };
    let mut titles: Vec<&str> = Vec::new();
    for title in file
        .canvas
        .iter()
        .map(|canvas| canvas.title.as_str())
        .chain([design_system.title.as_str()])
    {
        if title.trim().is_empty() || title.trim() != title {
            fail!(
                "{label}: {title:?} is no title; the publish finds each artifact by a title that has words and no space around them"
            );
        }
    }
    for canvas in &file.canvas {
        if titles.contains(&canvas.title.as_str()) {
            fail!(
                "{label} gives two canvases the title {:?}; the publish finds each canvas by its title",
                canvas.title
            );
        }
        titles.push(&canvas.title);
    }
    Ok(Published {
        canvases: file.canvas,
        design_system: design_system.title,
    })
}

/// Gives the one design system that <index>, the repo's `CANVAS`, names the
/// title <title> and the artifact link <url> that the publish found on the
/// account, so that every canvas names the design system it publishes beside.
/// Fails when the repo's entry gives either, since an artifact link belongs to
/// one account and `CANVASES` gives the title.
pub fn name_design_system(index: &mut Value, title: &str, url: &str) -> Result<()> {
    let Some([system]) = index["designSystems"].as_array_mut().map(Vec::as_mut_slice) else {
        fail!("{CANVAS} names no design system, or more than one; the boards load one");
    };
    let Some(system) = system.as_object_mut() else {
        fail!("{CANVAS}'s design system is no object");
    };
    for key in ["title", "artifact"] {
        if system.contains_key(key) {
            fail!(
                "{CANVAS}'s design system gives its {key}, which the publish sets from the artifact it finds on the account: remove the {key} from the entry"
            );
        }
    }
    system.insert("title".to_string(), json!(title));
    system.insert("artifact".to_string(), json!(url));
    Ok(())
}

/// What one canvas publishes: its index and the names of its boards.
#[derive(Debug)]
pub struct Split<'a> {
    pub canvas: &'a Canvas,
    pub index: Value,
    pub boards: Vec<String>,
}

/// A page of the canvas index: its entry in `pages`, and its id.
pub(crate) struct Page<'a> {
    pub entry: &'a Value,
    pub id: &'a str,
}

/// The pages of <index>, the repo's `CANVAS`, in its order; the first is the
/// page of every board and note that names none. Fails when the index lists
/// no page, or a page has no id.
pub(crate) fn pages_of(index: &Value) -> Result<Vec<Page<'_>>> {
    let Some(list) = index["pages"].as_array() else {
        fail!("{CANVAS} lists no pages");
    };
    let mut pages = Vec::new();
    for (n, entry) in list.iter().enumerate() {
        let Some(id) = entry["id"].as_str() else {
            fail!("{CANVAS}: page {} of `pages` ({entry}) has no id", n + 1);
        };
        pages.push(Page { entry, id });
    }
    if pages.is_empty() {
        fail!("{CANVAS} lists no pages");
    }
    Ok(pages)
}

/// The board entries that <index>, the repo's `CANVAS`, places, by board
/// file.
pub(crate) fn placed(index: &Value) -> Result<&Map<String, Value>> {
    match index["boards"].as_object() {
        Some(placed) => Ok(placed),
        None => fail!("{CANVAS}: `boards` is not an object of board files"),
    }
}

/// The notes that <index>, the repo's `CANVAS`, gives, by id; none when it
/// gives no `notes`.
pub(crate) fn notes_of(index: &Value) -> Result<Option<&Map<String, Value>>> {
    match index.get("notes") {
        None => Ok(None),
        Some(Value::Object(notes)) => Ok(Some(notes)),
        Some(_) => fail!("{CANVAS}: `notes` is not an object of notes"),
    }
}

/// The page that <entry>, the <kind> <name> of the index, is on: its own, or
/// the first page when it names none, as the canvas places it.
pub(crate) fn page_of<'a>(
    kind: &str,
    name: &str,
    entry: &'a Value,
    first: &'a str,
) -> Result<&'a str> {
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
    let page_list = pages_of(index)?;
    let pages: Vec<&str> = page_list.iter().map(|page| page.id).collect();
    let first = pages[0];
    let mut owner: BTreeMap<&str, usize> = BTreeMap::new();
    for (n, canvas) in canvases.iter().enumerate() {
        for page in &canvas.pages {
            if !pages.contains(&page.as_str()) {
                fail!(
                    "{CANVASES}: {:?} lists page {page}, which {CANVAS} does not list",
                    canvas.title
                );
            }
            if let Some(other) = owner.insert(page, n) {
                fail!(
                    "{CANVASES}: page {page} is on two canvases, {:?} and {:?}",
                    canvases[other].title,
                    canvas.title
                );
            }
        }
        if !canvas.pages.contains(&canvas.launch) {
            fail!(
                "{CANVASES}: {:?} launches on page {}, which is not one of its pages",
                canvas.title,
                canvas.launch
            );
        }
    }
    if let Some(page) = pages.iter().find(|page| !owner.contains_key(*page)) {
        fail!("{CANVASES}: page {page} is on no canvas; add it to the pages of one");
    }
    let placed = placed(index)?;
    if let Some(name) = boards.keys().find(|name| !placed.contains_key(*name)) {
        fail!("{name} is a board file that {CANVAS} does not place");
    }
    if let Some(name) = placed.keys().find(|name| !boards.contains_key(*name)) {
        fail!("{CANVAS} places {name}, which is no board file");
    }
    let no_notes = Map::new();
    let notes = notes_of(index)?.unwrap_or(&no_notes);
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
                    "{name} on {:?} mounts {file}, which is on {:?}; a board mounts only boards of its own canvas",
                    canvases[on].title,
                    canvases[other].title
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
                "{:?} would hold {files} files, and a canvas holds at most {MAX_FILES}; move a page to another canvas in {CANVASES}",
                canvas.title
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
                .filter(|page| owner.get(page.id) == Some(&n))
                .map(|page| page.entry.clone())
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
