//! The design system's assets on its claude.ai Design System artifact. The
//! artifact type keeps each picture, video or PDF under `assets/<group>/` as an
//! upload in the artifact's asset store, which the artifact's index names in a
//! record (`assetGroups.<group>.files.<key>`), and keeps a text file there
//! (such as a group's README.md) as an ordinary file. builds reads the repo's
//! `design/system/assets` at a commit, says which uploads differ from what the
//! live index records (by the sha256 that builds keeps in each record), and,
//! once the session has uploaded those, writes the index's records of every
//! asset at the commit and names the uploads that nothing references any more.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::design_system_artifact::{SYSTEM, files_under};
use crate::fail;
use crate::fail::{Context, Result};

/// The media type of each kind of file the artifact type keeps as an upload,
/// by its extension.
const UPLOADS: [(&str, &str); 9] = [
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("pdf", "application/pdf"),
];
/// The extensions of the text files the artifact type keeps under `assets/`
/// as ordinary files.
const TEXT: [&str; 4] = ["md", "json", "txt", "csv"];

/// One upload of the repo's design system at a commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// Its path on the artifact, `project/assets/<group>/<name>`.
    pub path: String,
    pub group: String,
    /// Its path below the group's folder.
    pub name: String,
    pub size: u64,
    pub media_type: String,
    /// The sha256 of its bytes, as hex.
    pub sha256: String,
}

/// An upload with its bytes.
pub type Upload = (Asset, Vec<u8>);

/// The assets of `SYSTEM` in <export>, a folder that holds it at its path:
/// the text files by their paths on the artifact, and each upload with its
/// bytes. A repo with no assets folder has none. Fails on a file directly
/// under `assets/`, which belongs to no group, and on a file the artifact
/// type keeps neither as an upload nor as text.
pub fn read(export: &Path) -> Result<(BTreeMap<String, String>, Vec<Upload>)> {
    let assets = export.join(SYSTEM).join("assets");
    let mut texts = BTreeMap::new();
    let mut uploads = Vec::new();
    if !assets.is_dir() {
        return Ok((texts, uploads));
    }
    for entry in std::fs::read_dir(&assets).context(|| format!("listing {SYSTEM}/assets"))? {
        let entry = entry.context(|| format!("listing {SYSTEM}/assets"))?;
        let group = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_dir() {
            fail!(
                "{SYSTEM}/assets/{group} is in no group; put it in a folder under {SYSTEM}/assets, which the Design System artifact shows as its group"
            );
        }
        for name in files_under(&entry.path())? {
            let from = entry.path().join(&name);
            let path = format!("project/assets/{group}/{name}");
            let extension = extension(&name);
            if TEXT.contains(&extension.as_str()) {
                let text = std::fs::read_to_string(&from)
                    .context(|| format!("reading {SYSTEM}/assets/{group}/{name}"))?;
                texts.insert(path, text);
            } else if let Some((_, media_type)) = UPLOADS.iter().find(|(e, _)| *e == extension) {
                let bytes = std::fs::read(&from)
                    .context(|| format!("reading {SYSTEM}/assets/{group}/{name}"))?;
                let asset = Asset {
                    path,
                    group: group.clone(),
                    name,
                    size: bytes.len() as u64,
                    media_type: media_type.to_string(),
                    sha256: crate::text::hex(&Sha256::digest(&bytes)),
                };
                uploads.push((asset, bytes));
            } else {
                fail!(
                    "{SYSTEM}/assets/{group}/{name} is neither a picture, video or PDF, which the Design System artifact keeps as an upload, nor a text file"
                );
            }
        }
    }
    uploads.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    Ok((texts, uploads))
}

/// The lowercase extension of the file <name>, empty when it has none.
pub fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

/// The key of the record of the upload <name> in its group's `files`: the
/// name with each byte outside `[A-Za-z0-9_./-]` written `~` and two hex
/// digits, as the artifact type keys them.
fn key(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-') {
                (b as char).to_string()
            } else {
                format!("~{b:02x}")
            }
        })
        .collect()
}

/// The asset records of a live index, read strictly: a key the index
/// leaves out is empty, and a key of the wrong shape fails, since builds
/// would otherwise write over what it cannot read.
#[derive(Debug, Default)]
struct Live {
    /// The groups in the index's order (`groups`).
    order: Vec<String>,
    groups: BTreeMap<String, LiveGroup>,
}

#[derive(Debug, Default)]
struct LiveGroup {
    /// The entry's keys other than `order` and `files`, kept as they are.
    entry: Map<String, Value>,
    /// The group's uploads in the index's order, by name.
    order: Vec<String>,
    /// Each record, by its key.
    files: BTreeMap<String, LiveRecord>,
}

#[derive(Debug)]
struct LiveRecord {
    blob: String,
    /// The sha256 builds wrote in the record; None in a record it did not
    /// write.
    sha256: Option<String>,
}

/// The value of <key> in <object>, which <label> names in an error, when it
/// is a list of strings; an empty list when <object> has no <key>.
fn strings(object: &Map<String, Value>, key: &str, label: &str) -> Result<Vec<String>> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        fail!("the index's {label} is {value}, which is no list of names");
    };
    items
        .iter()
        .map(|item| match item.as_str() {
            Some(name) => Ok(name.to_string()),
            None => fail!("the index's {label} holds {item}, which is no name"),
        })
        .collect()
}

/// The object that <value>, the index's <label>, is.
fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    match value.as_object() {
        Some(object) => Ok(object),
        None => fail!("the index's {label} is {value}, which is no object"),
    }
}

/// The asset records of <index>, the live index.
fn live(index: &Map<String, Value>) -> Result<Live> {
    let mut live = Live {
        order: strings(index, "groups", "groups")?,
        groups: BTreeMap::new(),
    };
    let Some(groups) = index.get("assetGroups") else {
        return Ok(live);
    };
    for (name, entry) in object(groups, "assetGroups")? {
        let label = format!("assetGroups.{name}");
        let entry = object(entry, &label)?;
        let mut group = LiveGroup {
            entry: entry.clone(),
            order: strings(entry, "order", &format!("{label}.order"))?,
            files: BTreeMap::new(),
        };
        group.entry.remove("order");
        group.entry.remove("files");
        if let Some(files) = entry.get("files") {
            for (key, record) in object(files, &format!("{label}.files"))? {
                let label = format!("{label}.files.{key}");
                let record = object(record, &label)?;
                let blob = match record.get("blob") {
                    Some(Value::String(blob)) => blob.clone(),
                    other => fail!(
                        "the index's {label} names the upload {}, which is no upload id",
                        other.unwrap_or(&Value::Null)
                    ),
                };
                let sha256 = match record.get("sha256") {
                    None => None,
                    Some(Value::String(sha256)) => Some(sha256.clone()),
                    Some(other) => {
                        fail!("the index's {label} gives the sha256 {other}, which is no hash")
                    }
                };
                group.files.insert(key.clone(), LiveRecord { blob, sha256 });
            }
        }
        live.groups.insert(name.clone(), group);
    }
    Ok(live)
}

impl Live {
    /// The upload that the record of <asset> names, when the record says the
    /// upload holds the asset's bytes.
    fn uploaded(&self, asset: &Asset) -> Option<&str> {
        let record = self
            .groups
            .get(&asset.group)?
            .files
            .get(&key(&asset.name))?;
        (record.sha256.as_deref() == Some(asset.sha256.as_str())).then_some(record.blob.as_str())
    }

    /// Every upload the records name.
    fn blobs(&self) -> BTreeSet<String> {
        self.groups
            .values()
            .flat_map(|group| group.files.values())
            .map(|record| record.blob.clone())
            .collect()
    }
}

/// The assets of <assets> that need an upload: those whose record in
/// <index>, the live index, is missing or names other bytes. Fails when the
/// index's asset records are of the wrong shape.
pub fn to_upload<'a>(index: &Map<String, Value>, assets: &'a [Asset]) -> Result<Vec<&'a Asset>> {
    let live = live(index)?;
    Ok(assets
        .iter()
        .filter(|asset| live.uploaded(asset).is_none())
        .collect())
}

/// The upload id in <url>, what the Artifact tool gave for one upload: the
/// part after `/_blob/` of an upload's url, or the id itself.
pub fn upload_id(url: &str) -> Result<String> {
    let id = url.rsplit_once("/_blob/").map_or(url, |(_, id)| id);
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        fail!("{url:?} is no upload's url or id");
    }
    Ok(id.to_string())
}

/// Sets the asset records of <index>, the live index, to <assets>: each
/// upload <new> names (path to upload id) or, for an asset the session did
/// not upload, the one the index's record names for the same bytes. Each
/// group keeps the other keys of its entry, and the groups and each group's
/// uploads keep the order the index gives them, with the new ones after,
/// by name; a group the commit lacks leaves the index. Returns the uploads
/// that the index named and no longer does, which the session deletes. Fails
/// when the index's asset records are of the wrong shape, or an asset has
/// neither a new upload nor a record of its bytes.
pub fn set_records(
    index: &mut Map<String, Value>,
    assets: &[Asset],
    new: &BTreeMap<String, String>,
) -> Result<Vec<String>> {
    let live = live(index)?;
    // Each group's records by key, and its uploads' names.
    let mut groups: BTreeMap<&str, (Map<String, Value>, Vec<String>)> = BTreeMap::new();
    let mut kept = BTreeSet::new();
    for asset in assets {
        let blob = match new.get(&asset.path) {
            Some(id) => id.clone(),
            None => match live.uploaded(asset) {
                Some(id) => id.to_string(),
                None => fail!(
                    "{} has no upload of its bytes: upload it, or write the files again with publish-files, which reads the index anew",
                    asset.path
                ),
            },
        };
        kept.insert(blob.clone());
        let (records, names) = groups.entry(&asset.group).or_default();
        records.insert(
            key(&asset.name),
            json!({
                "name": asset.name,
                "blob": blob,
                "size": asset.size,
                "type": asset.media_type,
                "sha256": asset.sha256,
            }),
        );
        names.push(asset.name.clone());
    }
    let order = kept_then_new(&live.order, groups.keys().map(|g| g.to_string()));
    let mut asset_groups = Map::new();
    for name in &order {
        let (records, names) = groups
            .remove(name.as_str())
            .expect("the order holds only these groups");
        let old = live.groups.get(name);
        let mut entry = old.map(|group| group.entry.clone()).unwrap_or_default();
        let old_order = old.map(|group| group.order.as_slice()).unwrap_or_default();
        entry.insert("name".to_string(), json!(name));
        entry.insert(
            "order".to_string(),
            json!(kept_then_new(old_order, names.into_iter())),
        );
        entry.insert("files".to_string(), Value::Object(records));
        asset_groups.insert(name.clone(), Value::Object(entry));
    }
    index.insert("groups".to_string(), json!(order));
    index.insert("assetGroups".to_string(), Value::Object(asset_groups));
    Ok(live.blobs().difference(&kept).cloned().collect())
}

/// <wanted> in the order <kept> gives them, each once, then the rest by
/// name.
fn kept_then_new(kept: &[String], wanted: impl Iterator<Item = String>) -> Vec<String> {
    let wanted: BTreeSet<String> = wanted.collect();
    let mut order: Vec<String> = Vec::new();
    for name in kept.iter().chain(&wanted) {
        if wanted.contains(name) && !order.contains(name) {
            order.push(name.clone());
        }
    }
    order
}

#[cfg(test)]
#[path = "design_system_assets_tests.rs"]
mod tests;
