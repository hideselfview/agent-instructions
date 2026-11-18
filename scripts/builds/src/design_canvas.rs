//! The files of the claude.ai canvases a repo publishes its boards to
//! (`builds design canvas-files`). A canvas holds at most `MAX_FILES` files,
//! so a repo with more boards publishes them to several canvases, split by
//! page: `design/mocks/canvases.toml` names each canvas's link, title,
//! launch page and pages. For each canvas, builds writes a folder that holds
//! the files the canvas publishes, at their paths on the canvas: its boards,
//! its part of `canvas.json`, and a copy of the design system under
//! `project/ds/<namespace>/`. Beside them it writes the batches that one
//! Artifact publish each sends. Only a Claude session can call the Artifact
//! tool, so builds publishes nothing itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use boards::{
    BOARD_SUFFIX, CANVAS, MOCKS, Source, TOKENS_JSON, design_namespace, export, imports, in_scratch,
};

use crate::fail;
use crate::fail::{Context, Fail, Result};

/// The file that names the canvases the boards publish to and the pages each
/// shows.
pub const CANVASES: &str = "design/mocks/canvases.toml";
/// Where a canvas keeps its index, the part of the repo's `CANVAS` it shows.
const PUBLISHED_INDEX: &str = "project/canvas.json";
/// How every canvas link starts; the canvas's id follows.
const LINK: &str = "https://claude.ai/artifact/";
/// The most files a canvas holds.
pub const MAX_FILES: usize = 512;
/// The most files one Artifact publish sends.
pub const BATCH: usize = 255;
/// The design system's files on a canvas, under `project/ds/<namespace>/`.
const DS_FILES: [&str; 4] = [
    "tokens.json",
    "tokens.css",
    "components/bundle.css",
    "components/bundle.js",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanvasesFile {
    #[serde(default)]
    canvas: Vec<Canvas>,
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

impl Canvas {
    /// The canvas's id, the end of its link.
    fn id(&self) -> &str {
        self.url
            .strip_prefix(LINK)
            .expect("parse_canvases accepts only canvas links")
    }
}

/// The canvases of <text>, a `CANVASES` file that <label> names in an error.
pub fn parse_canvases(text: &str, label: &str) -> Result<Vec<Canvas>> {
    let file: CanvasesFile =
        toml::from_str(text).map_err(|error| Fail::Message(format!("{label}: {error}")))?;
    if file.canvas.is_empty() {
        fail!(
            "{label} names no canvas: give each as a [[canvas]] table with its url, title, launch and pages"
        );
    }
    for (n, canvas) in file.canvas.iter().enumerate() {
        let id = canvas.url.strip_prefix(LINK).unwrap_or("");
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
            fail!(
                "{label}: {:?} is no canvas link; a canvas link is {LINK}<id>",
                canvas.url
            );
        }
        if file.canvas[..n].iter().any(|other| other.url == canvas.url) {
            fail!("{label} names the canvas {} twice", canvas.url);
        }
    }
    Ok(file.canvas)
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
        let files = mine.len() + 1 + DS_FILES.len();
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

/// <paths>, the files of one canvas, in the batches that one publish each
/// sends: the index first, then the design system, then the boards.
pub fn batches(mut paths: Vec<String>) -> Vec<Vec<String>> {
    let rank = |path: &str| {
        if path == PUBLISHED_INDEX {
            0
        } else if path.starts_with("project/ds/") {
            1
        } else {
            2
        }
    };
    paths.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    paths.chunks(BATCH).map(<[String]>::to_vec).collect()
}

/// One canvas's folder, as `write_canvases` wrote it.
#[derive(Debug)]
pub struct Written {
    pub url: String,
    /// The folder the canvas's files are in, at their paths on the canvas.
    pub root: PathBuf,
    /// How many files the canvas publishes.
    pub files: usize,
    /// The batch files, in the order they are published.
    pub batches: Vec<PathBuf>,
}

/// Writes, into the empty or new folder <out>, a folder per canvas of the
/// boards at <rev> of the repo <repo> is in: `<out>/<canvas id>/project/...`
/// and its batches `<out>/<canvas id>/batch-<n>.json`, each a JSON object of
/// the paths it publishes (each mapped to itself, as the Artifact tool's
/// `files` takes them under `root`). The design system's bundles are built
/// into <cache> (`Places::design_system_cache`). Reads no store and writes
/// nothing in the repo.
pub fn write_canvases(repo: &Path, rev: &str, out: &Path, cache: &Path) -> Result<Vec<Written>> {
    let from = Source::Commit { repo, rev };
    from.check()?;
    if std::fs::read_dir(out).is_ok_and(|mut entries| entries.next().is_some()) {
        fail!(
            "{} is not empty; give a new folder, so that no file of an earlier run is published",
            out.display()
        );
    }
    let index: Value = serde_json::from_str(&from.read(CANVAS, "the canvas index")?)
        .map_err(|error| Fail::Message(format!("{rev}:{CANVAS}: {error}")))?;
    let canvases = parse_canvases(
        &from.read(CANVASES, "the canvases file")?,
        &format!("{rev}:{CANVASES}"),
    )?;
    let boards = in_scratch(cache, |work| {
        export(repo, rev, &[MOCKS.to_string()], work, "the boards")?;
        read_boards(work).map_err(|error| boards::Error::new(error.text()))
    })?;
    let splits = split(&index, &canvases, &boards)?;
    let folder = design_namespace(&index)?;
    let [tokens_css, bundle_css, bundle_js] = from.design_system(cache)?;
    let tokens_json = from.read(TOKENS_JSON, "the design system's tokens")?;
    let system: BTreeMap<&str, &str> = DS_FILES
        .into_iter()
        .zip([
            tokens_json.as_str(),
            tokens_css.as_str(),
            bundle_css.as_str(),
            bundle_js.as_str(),
        ])
        .collect();
    let mut written = Vec::new();
    for split in splits {
        let root = out.join(split.canvas.id());
        let mut files: BTreeMap<String, &str> = BTreeMap::new();
        let index =
            serde_json::to_string_pretty(&split.index).expect("the index serializes") + "\n";
        files.insert(PUBLISHED_INDEX.to_string(), &index);
        for name in &split.boards {
            files.insert(format!("project/{name}"), &boards[name]);
        }
        for (rel, text) in &system {
            files.insert(format!("project/ds/{folder}/{rel}"), text);
        }
        for (path, text) in &files {
            let to = root.join(path);
            let dir = to.parent().expect("a file has a folder");
            std::fs::create_dir_all(dir).context(|| format!("making {}", dir.display()))?;
            std::fs::write(&to, text).context(|| format!("writing {}", to.display()))?;
        }
        let mut batch_files = Vec::new();
        for (n, batch) in batches(files.keys().cloned().collect()).iter().enumerate() {
            let map: Map<String, Value> = batch
                .iter()
                .map(|path| (path.clone(), json!(path)))
                .collect();
            let to = root.join(format!("batch-{}.json", n + 1));
            std::fs::write(
                &to,
                serde_json::to_string_pretty(&map).expect("a batch serializes") + "\n",
            )
            .context(|| format!("writing {}", to.display()))?;
            batch_files.push(to);
        }
        written.push(Written {
            url: split.canvas.url.clone(),
            root,
            files: files.len(),
            batches: batch_files,
        });
    }
    Ok(written)
}

/// The board files (name to text) of <export>, a folder that holds the
/// repo's design/mocks at its path.
fn read_boards(export: &Path) -> Result<BTreeMap<String, String>> {
    let from = Source::Dir(export);
    let mocks = export.join(MOCKS);
    let mut boards = BTreeMap::new();
    for entry in std::fs::read_dir(&mocks).context(|| format!("listing {}", mocks.display()))? {
        let entry = entry.context(|| format!("listing {}", mocks.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(BOARD_SUFFIX) && entry.path().is_file() {
            let text = from.read(&format!("{MOCKS}/{name}"), "the board")?;
            boards.insert(name, text);
        }
    }
    Ok(boards)
}

/// What `builds design canvas-files` prints: each canvas, its folder and its
/// batches, in the order a session publishes them.
pub fn report(written: &[Written]) -> String {
    let mut out = format!(
        "builds wrote the files of {} canvases. Publish each canvas with the Artifact tool as `builds help lists` (design canvas-files) says:\n",
        written.len()
    );
    for canvas in written {
        out.push_str(&format!(
            "\n{}: {} files in {}\n",
            canvas.url,
            canvas.files,
            canvas.root.display()
        ));
        for batch in &canvas.batches {
            out.push_str(&format!("  {}\n", batch.display()));
        }
    }
    out
}

#[cfg(test)]
#[path = "design_canvas_tests.rs"]
mod tests;
