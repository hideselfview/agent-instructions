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

use crate::design_render::{Source, export, imports};
use crate::design_tokens::TOKENS_JSON;
use crate::fail;
use crate::fail::{Context, Fail, Result};

/// The file that names the canvases and the pages each publishes.
pub const CANVASES: &str = "design/mocks/canvases.toml";
const MOCKS: &str = "design/mocks";
/// The canvas index, in the repo's design/mocks and on each canvas.
const INDEX: &str = "canvas.json";
const BOARD_SUFFIX: &str = ".dc.html";
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

/// The page that <entry> (a board or a note of the index) is on: its own,
/// or the first page when it names none, as the canvas places it.
fn page_of<'a>(entry: &'a Value, first: &'a str) -> &'a str {
    entry["page"].as_str().unwrap_or(first)
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
    let pages: Vec<&str> = index["pages"]
        .as_array()
        .map(|pages| pages.iter().filter_map(|p| p["id"].as_str()).collect())
        .unwrap_or_default();
    let Some(first) = pages.first().copied() else {
        fail!("{INDEX} lists no pages, and the canvases are split by page");
    };
    let mut owner: BTreeMap<&str, usize> = BTreeMap::new();
    for (n, canvas) in canvases.iter().enumerate() {
        for page in &canvas.pages {
            if !pages.contains(&page.as_str()) {
                fail!(
                    "{CANVASES}: {} lists page {page}, which {INDEX} does not list",
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
    let placed = index["boards"].as_object().cloned().unwrap_or_default();
    if let Some(name) = boards.keys().find(|name| !placed.contains_key(*name)) {
        fail!("{name} is a board file that {INDEX} does not place");
    }
    if let Some(name) = placed.keys().find(|name| !boards.contains_key(*name)) {
        fail!("{INDEX} places {name}, which is no board file");
    }
    let canvas_of = |name: &str| owner[page_of(&placed[name], first)];
    for (name, text) in boards {
        let on = canvas_of(name);
        for mounted in imports(text) {
            let file = format!("{mounted}{BOARD_SUFFIX}");
            if !placed.contains_key(&file) {
                fail!("{name} mounts {file}, which is no board file");
            }
            let other = canvas_of(&file);
            if other != on {
                fail!(
                    "{name} on {} mounts {file}, which is on {}; a board mounts only boards of its own canvas",
                    canvases[on].url,
                    canvases[other].url
                );
            }
        }
    }
    let notes = index["notes"].as_object().cloned().unwrap_or_default();
    let mut splits = Vec::new();
    for (n, canvas) in canvases.iter().enumerate() {
        let mine: Map<String, Value> = placed
            .iter()
            .filter(|(name, _)| canvas_of(name) == n)
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
            .filter(|(_, note)| owner[page_of(note, first)] == n)
            .map(|(id, note)| (id.clone(), note.clone()))
            .collect();
        let mut part = index.clone();
        part["title"] = json!(canvas.title);
        part["launch"] = json!({"view": "canvas", "page": canvas.launch});
        part["pages"] = Value::Array(
            index["pages"]
                .as_array()
                .expect("the index lists pages")
                .iter()
                .filter(|page| {
                    page["id"]
                        .as_str()
                        .is_some_and(|id| owner.get(id) == Some(&n))
                })
                .cloned()
                .collect(),
        );
        for (key, kept) in [("order", &mine), ("noteOrder", &my_notes)] {
            if let Some(order) = index[key].as_array() {
                part[key] = Value::Array(
                    order
                        .iter()
                        .filter(|entry| entry.as_str().is_some_and(|e| kept.contains_key(e)))
                        .cloned()
                        .collect(),
                );
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

/// The folder under `project/ds/` that the design system of <index> goes
/// in: the namespace of its one design system.
fn ds_folder(index: &Value) -> Result<String> {
    let systems = index["designSystems"].as_array().map(Vec::as_slice);
    let [system] = systems.unwrap_or_default() else {
        fail!("{INDEX} names no design system, or more than one; builds copies the repo's one");
    };
    let namespace = system["namespace"].as_str().unwrap_or("");
    let fine = namespace
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && namespace.len() <= 64
        && namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'));
    if !fine {
        fail!(
            "{INDEX}'s design system has the namespace {namespace:?}, which is no folder name on a canvas"
        );
    }
    Ok(namespace.to_string())
}

/// <paths>, the files of one canvas, in the batches that one publish each
/// sends: the index first, then the design system, then the boards.
pub fn batches(mut paths: Vec<String>) -> Vec<Vec<String>> {
    let rank = |path: &str| {
        if path == format!("project/{INDEX}") {
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

static WORK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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
    let work = cache.join(format!(
        ".canvas-{}-{}",
        std::process::id(),
        WORK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let read = (|| {
        export(repo, rev, &[MOCKS.to_string()], &work, "the boards")?;
        read_mocks(&work.join(MOCKS), rev)
    })();
    let _ = std::fs::remove_dir_all(&work);
    let (index, canvases, boards) = read?;
    let splits = split(&index, &canvases, &boards)?;
    let folder = ds_folder(&index)?;
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
        files.insert(format!("project/{INDEX}"), &index);
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

/// The index, the canvases and the board files (name to text).
type Mocks = (Value, Vec<Canvas>, BTreeMap<String, String>);

/// The `Mocks` of <mocks>, the repo's design/mocks at <rev> exported to disk.
fn read_mocks(mocks: &Path, rev: &str) -> Result<Mocks> {
    let read = |name: &str| -> Result<String> {
        let path = mocks.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fail!("{rev}:{MOCKS}/{name} is not there")
            }
            Err(error) => fail!("reading {}: {error}", path.display()),
        }
    };
    let index: Value = serde_json::from_str(&read(INDEX)?)
        .map_err(|error| Fail::Message(format!("{rev}:{MOCKS}/{INDEX}: {error}")))?;
    let canvases = parse_canvases(&read("canvases.toml")?, &format!("{rev}:{CANVASES}"))?;
    let mut boards = BTreeMap::new();
    for entry in std::fs::read_dir(mocks).context(|| format!("listing {}", mocks.display()))? {
        let entry = entry.context(|| format!("listing {}", mocks.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(BOARD_SUFFIX) && entry.path().is_file() {
            boards.insert(name.clone(), read(&name)?);
        }
    }
    Ok((index, canvases, boards))
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
