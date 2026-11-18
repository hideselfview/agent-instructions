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

use crate::design_system_artifact::SYSTEM;
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
        let mut found = Vec::new();
        files_under(&entry.path(), "", &mut found)?;
        for name in found {
            let from = entry.path().join(&name);
            let path = format!("project/assets/{group}/{name}");
            let extension = name
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_ascii_lowercase())
                .unwrap_or_default();
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
                    sha256: hex(&Sha256::digest(&bytes)),
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

/// Adds the path of every file under <dir>, below <prefix>, to <found>.
fn files_under(dir: &Path, prefix: &str, found: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).context(|| format!("listing {}", dir.display()))? {
        let entry = entry.context(|| format!("listing {}", dir.display()))?;
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        if entry.path().is_dir() {
            files_under(&entry.path(), &format!("{name}/"), found)?;
        } else {
            found.push(name);
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

/// The record of <asset> in <index>, if the index has one.
fn record<'a>(index: &'a Map<String, Value>, asset: &Asset) -> Option<&'a Value> {
    index
        .get("assetGroups")?
        .get(&asset.group)?
        .get("files")?
        .get(key(&asset.name))
}

/// The upload that <index>'s record of <asset> names, when the record says
/// the upload holds the asset's bytes.
fn uploaded<'a>(index: &'a Map<String, Value>, asset: &Asset) -> Option<&'a str> {
    let record = record(index, asset)?;
    (record.get("sha256").and_then(Value::as_str) == Some(asset.sha256.as_str()))
        .then(|| record.get("blob").and_then(Value::as_str))
        .flatten()
}

/// The assets of <assets> that need an upload: those whose record in
/// <index>, the live index, is missing or names other bytes.
pub fn to_upload<'a>(index: &Map<String, Value>, assets: &'a [Asset]) -> Vec<&'a Asset> {
    assets
        .iter()
        .filter(|asset| uploaded(index, asset).is_none())
        .collect()
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
/// when an asset has neither a new upload nor a record of its bytes.
pub fn set_records(
    index: &mut Map<String, Value>,
    assets: &[Asset],
    new: &BTreeMap<String, String>,
) -> Result<Vec<String>> {
    let before = blobs(index);
    let mut groups: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    for asset in assets {
        let blob = match new.get(&asset.path) {
            Some(id) => id.clone(),
            None => match uploaded(index, asset) {
                Some(id) => id.to_string(),
                None => fail!(
                    "{} has no upload of its bytes: upload it, or write the files again with publish-files, which reads the index anew",
                    asset.path
                ),
            },
        };
        groups.entry(&asset.group).or_default().insert(
            key(&asset.name),
            json!({
                "name": asset.name,
                "blob": blob,
                "size": asset.size,
                "type": asset.media_type,
                "sha256": asset.sha256,
            }),
        );
    }
    let old_groups = index
        .get("assetGroups")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let old_order: Vec<String> = index
        .get("groups")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let order = kept_then_new(&old_order, groups.keys().map(|g| g.to_string()));
    let mut asset_groups = Map::new();
    for group in &order {
        let files = &groups[group.as_str()];
        let mut entry = old_groups
            .get(group)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let old_files: Vec<String> = entry
            .get("order")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let names = files
            .values()
            .map(|record| record["name"].as_str().unwrap_or("").to_string());
        entry.insert("name".to_string(), json!(group));
        entry.insert("order".to_string(), json!(kept_then_new(&old_files, names)));
        entry.insert("files".to_string(), Value::Object(files.clone()));
        asset_groups.insert(group.clone(), Value::Object(entry));
    }
    index.insert("groups".to_string(), json!(order));
    index.insert("assetGroups".to_string(), Value::Object(asset_groups));
    let after = blobs(index);
    Ok(before.difference(&after).cloned().collect())
}

/// <wanted> in the order <kept> gives them, then the rest by name.
fn kept_then_new(kept: &[String], wanted: impl Iterator<Item = String>) -> Vec<String> {
    let wanted: BTreeSet<String> = wanted.collect();
    let mut order: Vec<String> = kept
        .iter()
        .filter(|name| wanted.contains(*name))
        .cloned()
        .collect();
    order.dedup();
    for name in wanted {
        if !order.contains(&name) {
            order.push(name);
        }
    }
    order
}

/// Every upload id the asset records of <index> name.
fn blobs(index: &Map<String, Value>) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let Some(groups) = index.get("assetGroups").and_then(Value::as_object) else {
        return found;
    };
    for group in groups.values() {
        let Some(files) = group.get("files").and_then(Value::as_object) else {
            continue;
        };
        for record in files.values() {
            if let Some(blob) = record.get("blob").and_then(Value::as_str) {
                found.insert(blob.to_string());
            }
        }
    }
    found
}

#[cfg(test)]
#[path = "design_system_assets_tests.rs"]
mod tests;
