//! The files that publish a repo's designs to claude.ai, all from one commit
//! (`builds design publish-files` and `publish-last`): each canvas that
//! `design/mocks/canvases.toml` names and the Design System artifact it names
//! beside them. Only a Claude session can call the Artifact tool, so builds
//! publishes nothing itself: it writes a folder per artifact with the
//! artifact's files at their paths on it, and the Artifact publish calls that
//! send them, which the session makes in order, with the uploads of the
//! design system's assets whose bytes the live index does not record. What
//! only the live artifacts hold (the files on each one now, the design
//! system's index, and the ids the uploads got) the session reads and gives
//! `publish-last`, which writes each artifact's last calls: the removal of
//! each file the commit no longer has, the design system's index with the
//! records of every asset at the commit, and the deletion of each upload the
//! index no longer names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use boards::runtime::RECORD as RUNTIME;
use boards::{CANVAS, MOCKS, Source, design_namespace, export, in_scratch};

use crate::design_canvas::{
    self as canvas, CANVASES, PUBLISHED_INDEX, Published, artifact_id, parse_canvases, split,
};
use crate::design_system_artifact::{self as system, SHARED_FILES};
use crate::design_system_assets::{self as assets, Asset, Upload};
use crate::fail;
use crate::fail::{Context, Fail, Result};

/// The most files one Artifact publish sends.
pub const BATCH: usize = 255;
/// The extensions of the text files the Artifact tool serves with the media
/// type it infers from the extension, so that a call names them by path.
const SERVED: [&str; 9] = [
    "html", "css", "js", "mjs", "json", "md", "txt", "csv", "svg",
];
/// The extensions of the source files the Artifact tool does not serve: a call
/// sends each as `text/plain`, which it must name.
const SOURCE: [&str; 7] = ["ts", "tsx", "jsx", "yaml", "yml", "toml", "map"];
/// The record in each artifact's folder of what the folder publishes.
const RECORD: &str = "artifact.json";

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
    /// the commit lacks leaves the artifact.
    fn repo_decides(self, path: &str) -> bool {
        match self {
            Kind::Canvas => canvas::repo_decides(path),
            Kind::DesignSystem => system::repo_decides(path),
        }
    }

    /// Where <path> goes among the artifact's calls: lower first.
    fn rank(self, path: &str) -> u8 {
        match self {
            Kind::Canvas => canvas::rank(path),
            Kind::DesignSystem => system::rank(path),
        }
    }

    /// The file that a call which only removes files names as its
    /// `file_path`: one the artifact publishes anyway.
    fn removal_anchor(self) -> &'static str {
        match self {
            Kind::Canvas => PUBLISHED_INDEX,
            Kind::DesignSystem => system::REMOVAL_ANCHOR,
        }
    }
}

/// What a folder publishes beside its files, by the kind of its artifact.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Holds {
    Canvas,
    DesignSystem {
        /// Every upload of the design system at the commit.
        assets: Vec<Asset>,
        /// The paths of the uploads that the session makes, those whose
        /// bytes the live index does not record.
        uploads: Vec<String>,
    },
}

impl Holds {
    fn kind(&self) -> Kind {
        match self {
            Holds::Canvas => Kind::Canvas,
            Holds::DesignSystem { .. } => Kind::DesignSystem,
        }
    }
}

/// What a folder publishes, which `publish-files` writes as `RECORD` beside
/// the files and `publish-last` reads.
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    url: String,
    /// The commit the files come from.
    commit: String,
    /// Every path the folder publishes as a file.
    paths: Vec<String>,
    #[serde(flatten)]
    holds: Holds,
}

/// What `write_all` wrote for one artifact beside its files and calls, by
/// the kind of the artifact.
#[derive(Debug)]
pub enum Artifact {
    Canvas,
    DesignSystem {
        /// The upload call files, each with the path on the artifact it
        /// uploads.
        uploads: Vec<(PathBuf, String)>,
    },
}

/// One artifact's folder, as `write_all` wrote it.
#[derive(Debug)]
pub struct Written {
    pub url: String,
    /// The folder the artifact's files are in, at their paths on it.
    pub root: PathBuf,
    /// How many files the artifact publishes.
    pub files: usize,
    /// The call files, in the order they are published.
    pub calls: Vec<PathBuf>,
    pub artifact: Artifact,
}

impl Written {
    pub fn kind(&self) -> Kind {
        match self.artifact {
            Artifact::Canvas => Kind::Canvas,
            Artifact::DesignSystem { .. } => Kind::DesignSystem,
        }
    }
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

/// The runtime's file and sha256 that <text>, the `RUNTIME` file that
/// <label> names, records.
fn parse_runtime(text: &str, label: &str) -> Result<(String, String)> {
    let record: boards::runtime::Record =
        toml::from_str(text).map_err(|error| Fail::Message(format!("{label}: {error}")))?;
    Ok((record.runtime, record.runtime_sha256))
}

/// Writes, into the empty or new folder <out>, a folder per artifact that
/// the designs at <rev> of the repo <repo> is in publish to, all from the
/// one commit <rev> names: `<out>/<artifact id>/project/...`, the record
/// `RECORD`, the calls `call-<n>.json`, and for the design system the
/// uploads `upload-<n>.json`: each file holds the arguments of one Artifact
/// call. The design system uploads each asset whose bytes <system_index>,
/// the text of its live index, records no upload of. The canvases come
/// first, in the order `CANVASES` names them, then the design system. The
/// design system's bundles are built into <cache>
/// (`Places::design_system_cache`). Reads no store and writes nothing in the
/// repo.
pub fn write_all(
    repo: &Path,
    rev: &str,
    out: &Path,
    cache: &Path,
    system_index: &str,
) -> Result<Publish> {
    let commit = Source::Commit { repo, rev }
        .check()?
        .expect("a commit source names a commit");
    let from = Source::Commit { repo, rev: &commit };
    if std::fs::read_dir(out).is_ok_and(|mut entries| entries.next().is_some()) {
        fail!(
            "{} is not empty; give a new folder, so that no file of an earlier run is published",
            out.display()
        );
    }
    let live_index = system::parse_index(system_index)?;
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
    let generated = from.design_system(cache)?;
    let mut inputs = from.design_system_inputs()?;
    inputs.push(MOCKS.to_string());
    let (boards, contents) = in_scratch(cache, |work| {
        export(repo, &commit, &inputs, work, "the designs")?;
        let boards = read_boards(work)?;
        let contents =
            system::contents(work, &generated).map_err(|error| boards::Error::new(error.text()))?;
        Ok((boards, contents))
    })?;
    let splits = split(&index, &canvases, &boards)?;
    let folder = design_namespace(&index)?;
    let copy: BTreeMap<String, String> = SHARED_FILES
        .into_iter()
        .map(|rel| {
            let text = &contents.files[&format!("project/{rel}")];
            (canvas::ds_copy(&folder, rel), text.clone())
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
        let (root, calls) =
            write_artifact(&out, &split.canvas.url, &commit, &files, Holds::Canvas)?;
        artifacts.push(Written {
            url: split.canvas.url.clone(),
            root,
            files: files.len(),
            calls,
            artifact: Artifact::Canvas,
        });
    }
    let all: Vec<Asset> = contents
        .uploads
        .iter()
        .map(|(asset, _)| asset.clone())
        .collect();
    let to_upload: Vec<String> = assets::to_upload(&live_index, &all)?
        .into_iter()
        .map(|asset| asset.path.clone())
        .collect();
    let (root, calls) = write_artifact(
        &out,
        &design_system,
        &commit,
        &contents.files,
        Holds::DesignSystem {
            assets: all,
            uploads: to_upload.clone(),
        },
    )?;
    let uploads = write_uploads(&root, &design_system, &contents.uploads, &to_upload)?;
    artifacts.push(Written {
        url: design_system.clone(),
        root,
        files: contents.files.len(),
        calls,
        artifact: Artifact::DesignSystem { uploads },
    });
    Ok(Publish {
        commit,
        artifacts,
        runtime,
        runtime_sha256,
    })
}

/// Writes the folder `<out>/<id>` of the artifact at <url>: <files> at their
/// paths, the record of them and of <holds>, and the calls that publish the
/// files. Returns the folder and the calls in order.
fn write_artifact(
    out: &Path,
    url: &str,
    commit: &str,
    files: &BTreeMap<String, String>,
    holds: Holds,
) -> Result<(PathBuf, Vec<PathBuf>)> {
    let root = out.join(artifact_id(url).expect("parse_canvases accepts only artifact links"));
    for (path, text) in files {
        write(&root.join(path), text)?;
    }
    let kind = holds.kind();
    let record = Record {
        url: url.to_string(),
        commit: commit.to_string(),
        paths: files.keys().cloned().collect(),
        holds,
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
        if content_type(first)?.is_some() {
            fail!(
                "{first} is the file that a call names as its file_path, which the Artifact tool serves only for a type it infers from the extension"
            );
        }
        let files = rest
            .iter()
            .map(|path| Ok((path.clone(), file_source(path)?)))
            .collect::<Result<Map<String, Value>>>()?;
        let to = root.join(format!("call-{}.json", n + 1));
        write(&to, &publish_call(url, &root, first, files))?;
        calls.push(to);
    }
    Ok((root, calls))
}

/// The media type that a call must name for the text file <path>: None for a
/// type the Artifact tool serves from the extension, `text/plain` for a source
/// file it does not serve. Fails on any other extension.
fn content_type(path: &str) -> Result<Option<&'static str>> {
    let extension = assets::extension(path);
    if SERVED.contains(&extension.as_str()) {
        Ok(None)
    } else if SOURCE.contains(&extension.as_str()) {
        Ok(Some("text/plain"))
    } else {
        fail!(
            "{path} is neither a file type the Artifact tool serves ({}) nor a source file ({}); add its extension to SERVED or SOURCE in design_publish.rs only when the tool serves it or it is text",
            SERVED.join(", "),
            SOURCE.join(", ")
        )
    }
}

/// The source of the file at <path> in a call's `files`: its path, or, for a
/// file the Artifact tool does not serve, the `{from, contentType}` form.
fn file_source(path: &str) -> Result<Value> {
    Ok(match content_type(path)? {
        None => json!(path),
        Some(media_type) => json!({"from": path, "contentType": media_type}),
    })
}

/// Writes into <root>, the design system's folder, the bytes of each of
/// <uploads> whose path <to_upload> names, and the call that uploads each to
/// the artifact at <url>. Returns each call with the path it uploads.
fn write_uploads(
    root: &Path,
    url: &str,
    uploads: &[Upload],
    to_upload: &[String],
) -> Result<Vec<(PathBuf, String)>> {
    let mut calls = Vec::new();
    for (asset, bytes) in uploads {
        if !to_upload.contains(&asset.path) {
            continue;
        }
        let file = root.join(&asset.path);
        let dir = file.parent().expect("a file has a folder");
        std::fs::create_dir_all(dir).context(|| format!("making {}", dir.display()))?;
        std::fs::write(&file, bytes).context(|| format!("writing {}", file.display()))?;
        let to = root.join(format!("upload-{}.json", calls.len() + 1));
        let call = json!({
            "action": "publish",
            "url": url,
            "asset": true,
            "file_path": file.display().to_string(),
        });
        write(&to, &json_text(&call))?;
        calls.push((to, asset.path.clone()));
    }
    Ok(calls)
}

/// The arguments of one Artifact publish to the artifact at <url> from
/// <root>, as JSON: it sends <file_path> (every publish names one) and
/// <files>, each path on the artifact mapped to its path under <root>, or to
/// null to remove it.
fn publish_call(url: &str, root: &Path, file_path: &str, files: Map<String, Value>) -> String {
    json_text(&json!({
        "action": "publish",
        "url": url,
        "root": root.display().to_string(),
        "file_path": root.join(file_path).display().to_string(),
        "files": files,
    }))
}

fn json_text(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("a call serializes") + "\n"
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
/// its folder, calls and uploads in the order a session makes them, and the
/// runtime check.
pub fn report(publish: &Publish) -> String {
    let canvases = publish
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind() == Kind::Canvas)
        .count();
    let mut out = format!(
        "builds wrote the files of {canvases} canvases and the design system at commit {}. Publish every artifact, in this order, as the publish-designs skill says:\n",
        publish.commit
    );
    for (n, artifact) in publish.artifacts.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. {} {}: {} files in {}\n",
            n + 1,
            artifact.kind().name(),
            artifact.url,
            artifact.files,
            artifact.root.display()
        ));
        for call in &artifact.calls {
            out.push_str(&format!("   {}\n", call.display()));
        }
        if let Artifact::DesignSystem { uploads } = &artifact.artifact {
            if uploads.is_empty() {
                out.push_str("   no upload: the index records every asset's bytes\n");
            }
            for (call, path) in uploads {
                out.push_str(&format!("   {} uploads {path}\n", call.display()));
            }
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
/// `- "`. Its other lines say nothing of a file. Fails on such a line with no
/// closing quote, which names no path builds can trust.
fn listed(listing: &str) -> Result<Vec<&str>> {
    let mut paths = Vec::new();
    for line in listing.lines() {
        let Some(rest) = line.trim_start().strip_prefix("- \"") else {
            continue;
        };
        let Some((path, _)) = rest.split_once('"') else {
            fail!("the listing line {line:?} opens a path's quotes and never closes them");
        };
        paths.push(path);
    }
    Ok(paths)
}

/// What the session gives `publish-last` for the design system: the text of
/// its live index, read after the uploads, who publishes, and the id each
/// upload got, by the upload's path on the artifact.
#[derive(Debug)]
pub struct SystemLast<'a> {
    pub index: &'a str,
    pub by: &'a str,
    pub uploaded: BTreeMap<String, String>,
}

/// The last calls of one artifact, as `write_last` wrote them.
#[derive(Debug, PartialEq)]
pub struct LastCalls {
    /// The publish calls, in order.
    pub calls: Vec<PathBuf>,
    /// The calls that delete an upload, made after the last publish call.
    pub deletes: Vec<PathBuf>,
}

/// Writes the last calls of the artifact whose folder is <root>, as
/// `publish-files` wrote it: `last-<n>.json`, which remove each file that
/// <listing> (the artifact's files now, as the Artifact tool lists them)
/// shows, the repo decides, and the commit lacks, and, for the design
/// system, send its index last. The index is <system>'s, with a record of
/// each asset at the commit and its `lastChange` saying that <system>'s
/// `by` published the commit at <now>; `delete-<n>.json` then delete each
/// upload the live index named and the new one does not. A canvas takes no
/// <system>, and the design system needs one, whose uploads must be the ones
/// the folder's upload calls make. Replaces the last calls of an earlier run
/// in the folder.
pub fn write_last(
    root: &Path,
    listing: &str,
    system_last: Option<SystemLast>,
    now: &str,
) -> Result<LastCalls> {
    let root = &std::fs::canonicalize(root).context(|| format!("finding {}", root.display()))?;
    let record_file = root.join(RECORD);
    let record: Record = serde_json::from_str(
        &std::fs::read_to_string(&record_file)
            .context(|| format!("reading {}", record_file.display()))?,
    )
    .map_err(|error| Fail::Message(format!("{}: {error}", record_file.display())))?;
    let shown = listed(listing)?;
    if shown.is_empty() {
        fail!(
            "the listing names no file; give the Artifact tool's listing of {}'s files, each line as `- \"<path>\" ...`",
            record.url
        );
    }
    let gone: Vec<&str> = shown
        .into_iter()
        .filter(|path| {
            record.holds.kind().repo_decides(path) && !record.paths.iter().any(|p| p == path)
        })
        .collect();
    for entry in std::fs::read_dir(root).context(|| format!("listing {}", root.display()))? {
        let entry = entry.context(|| format!("listing {}", root.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if (name.starts_with("last-") || name.starts_with("delete-")) && name.ends_with(".json") {
            let path = entry.path();
            std::fs::remove_file(&path).context(|| format!("removing {}", path.display()))?;
        }
    }
    let anchor = record.holds.kind().removal_anchor();
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
    let mut deletes = Vec::new();
    match (&record.holds, system_last) {
        (Holds::Canvas, None) => {}
        (Holds::Canvas, Some(_)) => fail!("{} is a canvas, which takes no index", record.url),
        (Holds::DesignSystem { .. }, None) => fail!(
            "{} is the design system, whose index goes in its last call: read {} from it and give it with --index, and who publishes with --by",
            record.url,
            system::INDEX
        ),
        (
            Holds::DesignSystem {
                assets: all,
                uploads,
            },
            Some(last),
        ) => {
            let made: Vec<&String> = last.uploaded.keys().collect();
            if made != uploads.iter().collect::<Vec<_>>() {
                fail!(
                    "--uploaded names the uploads of {}, and the folder's upload calls make those of {}: give the id that each upload call got, and only those",
                    list(made.iter().map(|p| p.as_str())),
                    list(uploads.iter().map(String::as_str))
                );
            }
            let mut index = system::parse_index(last.index)?;
            let unused = assets::set_records(&mut index, all, &last.uploaded)?;
            system::set_last_change(&mut index, last.by, now, &record.commit);
            write(&root.join(system::INDEX), &system::index_text(&index))?;
            calls.push((system::INDEX, Map::new()));
            for (n, id) in unused.iter().enumerate() {
                let to = root.join(format!("delete-{}.json", n + 1));
                let call = json!({"action": "delete", "url": record.url, "path": id});
                write(&to, &json_text(&call))?;
                deletes.push(to);
            }
        }
    }
    let mut written = Vec::new();
    for (n, (file_path, files)) in calls.into_iter().enumerate() {
        let to = root.join(format!("last-{}.json", n + 1));
        write(&to, &publish_call(&record.url, root, file_path, files))?;
        written.push(to);
    }
    Ok(LastCalls {
        calls: written,
        deletes,
    })
}

/// <items> as a list a sentence names, or "none".
fn list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let items: Vec<&str> = items.collect();
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

/// What `builds design publish-last` prints: the calls to make, or that the
/// artifact has nothing left to send.
pub fn last_report(root: &Path, last: &LastCalls) -> String {
    if last.calls.is_empty() {
        return format!(
            "{} has no file the commit lacks, so it needs no last call.\n",
            root.display()
        );
    }
    let mut out = format!(
        "builds wrote the last calls of {}. Make them in this order:\n",
        root.display()
    );
    for call in last.calls.iter().chain(&last.deletes) {
        out.push_str(&format!("  {}\n", call.display()));
    }
    out
}

#[cfg(test)]
#[path = "design_publish_tests.rs"]
mod tests;
