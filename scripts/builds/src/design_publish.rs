//! The files that publish a repo's designs to claude.ai, all from one commit
//! (`builds design publish-files` and `publish-last`): each canvas that
//! `design/mocks/canvases.toml` names and the Design System artifact it names
//! beside them. Only a Claude session can call the Artifact tool, so builds
//! publishes nothing itself: it writes a folder per artifact with the
//! artifact's files at their paths on it, and the Artifact publish calls that
//! send them, which the session makes in order. What only the live artifacts
//! hold (the files on each one now, and the design system's index) the
//! session reads and gives `publish-last`, which writes each artifact's last
//! calls: the removal of each file the commit no longer has, and the design
//! system's index.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use boards::{CANVAS, MOCKS, Source, design_namespace, export, git, git_text, in_scratch};

use crate::design_canvas::{
    CANVASES, DS_FILES, PUBLISHED_INDEX, Published, artifact_id, parse_canvases, split,
};
use crate::design_system_artifact::{self as system, SYSTEM};
use crate::fail;
use crate::fail::{Context, Fail, Result};

/// The most files one Artifact publish sends.
pub const BATCH: usize = 255;
/// The record of the canvas runtime the boards are drawn with.
const RUNTIME: &str = "design/mocks/runtime.toml";
/// The record in each artifact's folder of what the folder publishes.
const RECORD: &str = "artifact.json";
/// The file a call that only removes files names as its `file_path`, which
/// every publish needs: one the artifact publishes anyway.
const DESIGN_SYSTEM_README: &str = "project/README.md";

/// Which kind of artifact a folder publishes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Canvas,
    DesignSystem,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Canvas => "canvas",
            Kind::DesignSystem => "design system",
        }
    }

    /// Whether the repo decides the artifact's file at <path>, so that a path
    /// the commit lacks leaves the artifact. On a canvas, the repo decides
    /// every file under `project/`.
    fn repo_decides(self, path: &str) -> bool {
        match self {
            Kind::Canvas => path.starts_with("project/"),
            Kind::DesignSystem => system::repo_decides(path),
        }
    }

    /// Where <path> goes among the artifact's calls: lower first. A canvas
    /// gets its index first, then the design system, then the boards. A
    /// design system gets its tokens and bundles first, which every preview
    /// loads, and its cover last, as the artifact type asks.
    fn rank(self, path: &str) -> u8 {
        match self {
            Kind::Canvas if path == PUBLISHED_INDEX => 0,
            Kind::Canvas if path.starts_with("project/ds/") => 1,
            Kind::Canvas => 2,
            Kind::DesignSystem
                if matches!(
                    path,
                    "project/tokens.json"
                        | "project/tokens.css"
                        | "project/components/bundle.css"
                        | "project/components/bundle.js"
                ) =>
            {
                0
            }
            Kind::DesignSystem if path.starts_with("project/components/Cover/") => 2,
            Kind::DesignSystem => 1,
        }
    }
}

/// What a folder publishes, which `publish-files` writes as `RECORD` beside
/// the files and `publish-last` reads.
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    url: String,
    kind: Kind,
    /// The commit the files come from.
    commit: String,
    /// Every path the folder publishes.
    paths: Vec<String>,
}

/// One artifact's folder, as `write_all` wrote it.
#[derive(Debug)]
pub struct Written {
    pub url: String,
    pub kind: Kind,
    /// The folder the artifact's files are in, at their paths on it.
    pub root: PathBuf,
    /// How many files the artifact publishes.
    pub files: usize,
    /// The call files, in the order they are published.
    pub calls: Vec<PathBuf>,
}

/// Everything `write_all` wrote, for one commit.
#[derive(Debug)]
pub struct Publish {
    pub commit: String,
    pub artifacts: Vec<Written>,
    /// The canvas runtime's file on a canvas, and the sha256 that `RUNTIME`
    /// records for it at the commit.
    pub runtime: String,
    pub runtime_sha256: String,
}

/// The commit that <rev> of the repo <repo> is in names.
fn resolve(repo: &Path, rev: &str) -> Result<String> {
    let sha = git_text(
        git(repo).args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ]),
        &format!("find the commit {rev}"),
    )?;
    Ok(sha.trim().to_string())
}

/// The runtime's file and sha256 that <text>, the `RUNTIME` file that
/// <label> names, records.
fn parse_runtime(text: &str, label: &str) -> Result<(String, String)> {
    let table: toml::Table =
        toml::from_str(text).map_err(|error| Fail::Message(format!("{label}: {error}")))?;
    let field = |key: &str| match table.get(key) {
        Some(toml::Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => fail!("{label} gives no {key}"),
    };
    Ok((field("runtime")?, field("runtime_sha256")?))
}

/// Writes, into the empty or new folder <out>, a folder per artifact that
/// the designs at <rev> of the repo <repo> is in publish to, all from the
/// one commit <rev> names: `<out>/<artifact id>/project/...`, the record
/// `RECORD`, and the calls `call-<n>.json`, each the arguments of one
/// Artifact publish but its `action`. The canvases come first, in the order
/// `CANVASES` names them, then the design system. The design system's
/// bundles are built into <cache> (`Places::design_system_cache`). Reads no
/// store and writes nothing in the repo.
pub fn write_all(repo: &Path, rev: &str, out: &Path, cache: &Path) -> Result<Publish> {
    let commit = resolve(repo, rev)?;
    let from = Source::Commit { repo, rev: &commit };
    if std::fs::read_dir(out).is_ok_and(|mut entries| entries.next().is_some()) {
        fail!(
            "{} is not empty; give a new folder, so that no file of an earlier run is published",
            out.display()
        );
    }
    let index: Value = serde_json::from_str(&from.read(CANVAS, "the canvas index")?)
        .map_err(|error| Fail::Message(format!("{commit}:{CANVAS}: {error}")))?;
    let Published {
        canvases,
        design_system,
    } = parse_canvases(
        &from.read(CANVASES, "the canvases file")?,
        &format!("{commit}:{CANVASES}"),
    )?;
    let (runtime, runtime_sha256) = parse_runtime(
        &from.read(RUNTIME, "the canvas runtime's record")?,
        &format!("{commit}:{RUNTIME}"),
    )?;
    let [tokens_css, bundle_css, bundle_js] = from.design_system(cache)?;
    let (boards, system_files) = in_scratch(cache, |work| {
        export(
            repo,
            &commit,
            &[MOCKS.to_string(), SYSTEM.to_string()],
            work,
            "the designs",
        )?;
        let boards = read_boards(work)?;
        let system_files = system::files(work, &tokens_css, &bundle_css, &bundle_js)
            .map_err(|error| boards::Error::new(error.text()))?;
        Ok((boards, system_files))
    })?;
    let splits = split(&index, &canvases, &boards)?;
    let folder = design_namespace(&index)?;
    let copy: BTreeMap<String, String> = DS_FILES
        .into_iter()
        .map(|rel| {
            let text = &system_files[&format!("project/{rel}")];
            (format!("project/ds/{folder}/{rel}"), text.clone())
        })
        .collect();
    std::fs::create_dir_all(out).context(|| format!("making {}", out.display()))?;
    let out = std::fs::canonicalize(out).context(|| format!("finding {}", out.display()))?;
    let mut artifacts = Vec::new();
    for split in splits {
        let mut files = copy.clone();
        let index =
            serde_json::to_string_pretty(&split.index).expect("the index serializes") + "\n";
        files.insert(PUBLISHED_INDEX.to_string(), index);
        for name in &split.boards {
            files.insert(format!("project/{name}"), boards[name].clone());
        }
        artifacts.push(write_artifact(
            &out,
            &split.canvas.url,
            Kind::Canvas,
            &commit,
            &files,
        )?);
    }
    artifacts.push(write_artifact(
        &out,
        &design_system,
        Kind::DesignSystem,
        &commit,
        &system_files,
    )?);
    Ok(Publish {
        commit,
        artifacts,
        runtime,
        runtime_sha256,
    })
}

/// Writes the folder `<out>/<id>` of the artifact at <url>: <files> at their
/// paths, the record, and the calls that publish them.
fn write_artifact(
    out: &Path,
    url: &str,
    kind: Kind,
    commit: &str,
    files: &BTreeMap<String, String>,
) -> Result<Written> {
    let root = out.join(artifact_id(url).expect("parse_canvases accepts only artifact links"));
    for (path, text) in files {
        write(&root.join(path), text)?;
    }
    let record = Record {
        url: url.to_string(),
        kind,
        commit: commit.to_string(),
        paths: files.keys().cloned().collect(),
    };
    write(
        &root.join(RECORD),
        &(serde_json::to_string_pretty(&record).expect("the record serializes") + "\n"),
    )?;
    let mut paths: Vec<String> = files.keys().cloned().collect();
    paths.sort_by(|a, b| kind.rank(a).cmp(&kind.rank(b)).then_with(|| a.cmp(b)));
    let mut calls = Vec::new();
    for (n, batch) in paths.chunks(BATCH).enumerate() {
        let [first, rest @ ..] = batch else {
            unreachable!("a chunk holds a path")
        };
        let files: Map<String, Value> = rest
            .iter()
            .map(|path| (path.clone(), json!(path)))
            .collect();
        let to = root.join(format!("call-{}.json", n + 1));
        write(&to, &call(url, &root, first, files))?;
        calls.push(to);
    }
    Ok(Written {
        url: url.to_string(),
        kind,
        root,
        files: files.len(),
        calls,
    })
}

/// The arguments of one Artifact publish to the artifact at <url> from
/// <root>, as JSON: it sends <file_path> (every publish names one) and
/// <files>, each path on the artifact mapped to its path under <root>, or to
/// null to remove it.
fn call(url: &str, root: &Path, file_path: &str, files: Map<String, Value>) -> String {
    let call = json!({
        "url": url,
        "root": root.display().to_string(),
        "file_path": root.join(file_path).display().to_string(),
        "files": files,
    });
    serde_json::to_string_pretty(&call).expect("a call serializes") + "\n"
}

fn write(to: &Path, text: &str) -> Result<()> {
    let dir = to.parent().expect("a file has a folder");
    std::fs::create_dir_all(dir).context(|| format!("making {}", dir.display()))?;
    std::fs::write(to, text).context(|| format!("writing {}", to.display()))
}

/// The board files (name to text) of <export>, a folder that holds the
/// repo's design/mocks at its path.
fn read_boards(export: &Path) -> std::result::Result<BTreeMap<String, String>, boards::Error> {
    let from = Source::Dir(export);
    let mocks = export.join(MOCKS);
    let listing =
        |error: std::io::Error| boards::Error::new(format!("listing {}: {error}", mocks.display()));
    let mut boards = BTreeMap::new();
    for entry in std::fs::read_dir(&mocks).map_err(listing)? {
        let entry = entry.map_err(listing)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(boards::BOARD_SUFFIX) && entry.path().is_file() {
            let text = from.read(&format!("{MOCKS}/{name}"), "the board")?;
            boards.insert(name, text);
        }
    }
    Ok(boards)
}

/// What `builds design publish-files` prints: the commit, each artifact with
/// its folder and calls in the order a session makes them, and the runtime
/// check.
pub fn report(publish: &Publish) -> String {
    let canvases = publish
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == Kind::Canvas)
        .count();
    let mut out = format!(
        "builds wrote the files of {canvases} canvases and the design system at commit {}. Publish every artifact, in this order, as the publish-designs skill says:\n",
        publish.commit
    );
    for (n, artifact) in publish.artifacts.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. {} {}: {} files in {}\n",
            n + 1,
            artifact.kind.name(),
            artifact.url,
            artifact.files,
            artifact.root.display()
        ));
        for call in &artifact.calls {
            out.push_str(&format!("   {}\n", call.display()));
        }
    }
    out.push_str(&format!(
        "\nThen read {} from each canvas: its sha256 must be {}, as {RUNTIME} records at {}.\n",
        publish.runtime, publish.runtime_sha256, publish.commit
    ));
    out
}

/// The paths that <listing>, the Artifact tool's listing of an artifact's
/// files, names: the text between the quotes of each line that starts with
/// `- "`. Its other lines say nothing of a file.
fn listed(listing: &str) -> Vec<&str> {
    listing
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("- \""))
        .filter_map(|rest| rest.split_once('"').map(|(path, _)| path))
        .collect()
}

/// Writes the last calls of the artifact whose folder is <root>, as
/// `publish-files` wrote it: `last-<n>.json`, which remove each file that
/// <listing> (the artifact's files now, as the Artifact tool lists them)
/// shows, the repo decides, and the commit lacks, and, for the design
/// system, send its index last: <index>, the text of the live index, with
/// its `lastChange` saying that <by> published the commit at <now>. A canvas
/// takes no index, and the design system needs one. Replaces the last calls
/// of an earlier run in the folder, and returns the new ones in order.
pub fn write_last(
    root: &Path,
    listing: &str,
    index: Option<(&str, &str)>,
    now: &str,
) -> Result<Vec<PathBuf>> {
    let root = &std::fs::canonicalize(root).context(|| format!("finding {}", root.display()))?;
    let record_file = root.join(RECORD);
    let record: Record = serde_json::from_str(
        &std::fs::read_to_string(&record_file)
            .context(|| format!("reading {}", record_file.display()))?,
    )
    .map_err(|error| Fail::Message(format!("{}: {error}", record_file.display())))?;
    let shown = listed(listing);
    if shown.is_empty() {
        fail!(
            "the listing names no file; give the Artifact tool's listing of {}'s files, each line as `- \"<path>\" ...`",
            record.url
        );
    }
    let gone: Vec<&str> = shown
        .into_iter()
        .filter(|path| record.kind.repo_decides(path) && !record.paths.iter().any(|p| p == path))
        .collect();
    for entry in std::fs::read_dir(root).context(|| format!("listing {}", root.display()))? {
        let path = entry
            .context(|| format!("listing {}", root.display()))?
            .path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with("last-") && name.ends_with(".json") {
            std::fs::remove_file(&path).context(|| format!("removing {}", path.display()))?;
        }
    }
    let anchor = match record.kind {
        Kind::Canvas => PUBLISHED_INDEX,
        Kind::DesignSystem => DESIGN_SYSTEM_README,
    };
    let mut calls: Vec<(&str, Map<String, Value>)> = gone
        .chunks(BATCH - 1)
        .map(|chunk| {
            (
                anchor,
                chunk
                    .iter()
                    .map(|path| (path.to_string(), Value::Null))
                    .collect(),
            )
        })
        .collect();
    match (record.kind, index) {
        (Kind::Canvas, None) => {}
        (Kind::Canvas, Some(_)) => fail!("{} is a canvas, which takes no index", record.url),
        (Kind::DesignSystem, None) => fail!(
            "{} is the design system, whose index goes in its last call: read {} from it and give it with --index, and who publishes with --by",
            record.url,
            system::INDEX
        ),
        (Kind::DesignSystem, Some((text, by))) => {
            write(
                &root.join(system::INDEX),
                &system::with_last_change(text, by, now, &record.commit)?,
            )?;
            calls.push((system::INDEX, Map::new()));
        }
    }
    let mut written = Vec::new();
    for (n, (file_path, files)) in calls.into_iter().enumerate() {
        let to = root.join(format!("last-{}.json", n + 1));
        write(&to, &call(&record.url, root, file_path, files))?;
        written.push(to);
    }
    Ok(written)
}

/// What `builds design publish-last` prints: the calls to make, or that the
/// artifact has nothing left to send.
pub fn last_report(root: &Path, calls: &[PathBuf]) -> String {
    if calls.is_empty() {
        return format!(
            "{} has no file the commit lacks, so it needs no last call.\n",
            root.display()
        );
    }
    let mut out = format!(
        "builds wrote the last calls of {}. Make them in this order:\n",
        root.display()
    );
    for call in calls {
        out.push_str(&format!("  {}\n", call.display()));
    }
    out
}

#[cfg(test)]
#[path = "design_publish_tests.rs"]
mod tests;
