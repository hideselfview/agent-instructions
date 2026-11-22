//! What each build root's folders take, as the last command that wrote them
//! measured it. macOS keeps no file events for the build volume (its
//! `.fseventsd/no_log` file turns them off, `fsevents_log.rs`), so nothing
//! watches the volume. A command that writes build folders measures them when
//! it ends and writes the answer to `<daemon dir>/sizes/<repo>/<checkout>.json`,
//! then tells the dashboard (`daemon::client::wrote`). The dashboard and
//! `builds status` read those records, and measure a folder that has none once.
//!
//! A record holds one `Measure` for each of the root's manifest folders, keyed
//! by the folder's path relative to the root, or one under the key "" for the
//! whole of a root whose checkout is gone. A write that no builds command made
//! (a cargo run by hand) shows in a record only after the next command that
//! measures the root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::capped_store::entries_in;
use crate::checkout::Root;
use crate::fail::{Context, Result};
use crate::manifest::build_dirs;
use crate::places::Places;
use crate::usage::{Measure, measure_many};

/// A root's folders and what they take.
pub type Sizes = BTreeMap<String, Measure>;

/// The key of a gone root's whole size.
const WHOLE: &str = "";

/// The root at <path>: the build root that holds it, `<volume>/<repo>/<checkout>`;
/// None for a path outside the build volume or above its roots.
pub fn root_of(places: &Places, path: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(&places.root).ok()?;
    let mut parts = rest.components();
    let repo = parts.next()?;
    let checkout = parts.next()?;
    Some(places.root.join(repo).join(checkout))
}

/// A deletion is about to change a folder of <path>'s root: the root's record
/// is removed, which makes the dashboard measure it once, and the daemon is
/// told. A path outside the roots has no record.
pub fn stale(places: &Places, path: &Path) -> Result<()> {
    let Some(root) = root_of(places, path) else {
        return Ok(());
    };
    remove(places, &root)?;
    crate::daemon::client::wrote(places, &[root]);
    Ok(())
}

/// The file that holds <root>'s record.
fn record_file(places: &Places, root: &Path) -> Result<PathBuf> {
    let name = root
        .strip_prefix(&places.root)
        .ok()
        .filter(|rest| rest.components().count() == 2)
        .and_then(|rest| rest.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let (Some(name), Some(repo)) = (name, root.parent().and_then(Path::file_name)) else {
        crate::fail!(
            "{} is not a build root of {}",
            root.display(),
            places.root.display()
        );
    };
    Ok(places
        .daemon_dir()
        .join("sizes")
        .join(repo)
        .join(format!("{name}.json")))
}

/// <root>'s record; empty when it has none.
pub fn read(places: &Places, root: &Path) -> Result<Sizes> {
    crate::readings_file::read_value(&record_file(places, root)?)
}

/// Replaces <root>'s record with <sizes>, whole.
pub fn write(places: &Places, root: &Path, sizes: &Sizes) -> Result<()> {
    crate::readings_file::write_value(&record_file(places, root)?, sizes)
}

/// Removes <root>'s record.
fn remove(places: &Places, root: &Path) -> Result<()> {
    crate::whole_file::remove_if_present(&record_file(places, root)?)
}

/// The keys of <root>'s record: its manifest folders while its checkout is
/// there, else the whole root.
pub fn keys_of(root: &Root) -> Result<Vec<String>> {
    match &root.live {
        Some(checkout) => Ok(build_dirs(&checkout.top)?
            .into_iter()
            .map(|entry| entry.path)
            .collect()),
        None => Ok(vec![WHOLE.to_string()]),
    }
}

/// The path a key of <root>'s record names.
pub fn path_of(root: &Path, key: &str) -> PathBuf {
    if key == WHOLE {
        root.to_path_buf()
    } else {
        root.join(key)
    }
}

/// Measures <keys> of <root> now.
fn measure_keys(root: &Path, keys: &[String]) -> Result<Vec<Measure>> {
    let paths: Vec<PathBuf> = keys.iter().map(|key| path_of(root, key)).collect();
    measure_many(&paths)
}

/// <root>'s sizes for <keys>, from its record where it has them and measured
/// where it has not; what was measured is added to the record.
pub fn known(places: &Places, root: &Root, keys: &[String]) -> Result<Vec<Measure>> {
    let mut sizes = read(places, &root.path)?;
    let missing: Vec<String> = keys
        .iter()
        .filter(|key| !sizes.contains_key(*key))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let measured = measure_keys(&root.path, &missing)?;
        sizes.extend(missing.into_iter().zip(measured));
        write(places, &root.path, &sizes)?;
    }
    Ok(keys.iter().map(|key| sizes[key]).collect())
}

/// Measures every folder of the root at <path> and writes its record; removes
/// the record of a path that is no root. Returns whether the root exists.
pub fn record(places: &Places, path: &Path) -> Result<bool> {
    let Some(root) = Root::read(places, path)? else {
        remove(places, path)?;
        return Ok(false);
    };
    let keys = keys_of(&root)?;
    let measured = measure_keys(path, &keys)?;
    write(places, path, &keys.into_iter().zip(measured).collect())?;
    Ok(true)
}

/// The record for each of <paths>, for the paths that are a folder a record
/// keeps; None for any other, which the caller measures.
pub fn recorded(places: &Places, paths: &[PathBuf]) -> Result<Vec<Option<Measure>>> {
    let mut records: BTreeMap<PathBuf, Sizes> = BTreeMap::new();
    let mut answers = Vec::with_capacity(paths.len());
    for path in paths {
        let Some(root) = root_of(places, path) else {
            answers.push(None);
            continue;
        };
        if !records.contains_key(&root) {
            records.insert(root.clone(), read(places, &root)?);
        }
        let key = match path.strip_prefix(&root) {
            Ok(rest) if rest.as_os_str().is_empty() => WHOLE.to_string(),
            Ok(rest) => rest.to_string_lossy().into_owned(),
            Err(_) => {
                answers.push(None);
                continue;
            }
        };
        answers.push(records[&root].get(&key).copied());
    }
    Ok(answers)
}

/// After `builds <command> <args>` ran: measures the build roots it wrote
/// and tells the dashboard. A failure is said and does not change the
/// command's result, since the command's own work is done.
pub fn record_after(places: &Places, command: &str, args: &[String]) {
    let roots = match written_by(places, command, args) {
        Ok(roots) => roots,
        Err(error) => {
            crate::text::note(format!("sizes: {}", error.text()));
            Vec::new()
        }
    };
    if command == "prune"
        && let Err(error) = forget_unrooted(places)
    {
        crate::text::note(format!("sizes: {}", error.text()));
    }
    record_roots(places, &roots);
}

/// Removes the records of roots that are gone: a record has no other owner,
/// and a root removed by another builds process or by hand leaves its
/// record behind.
fn forget_unrooted(places: &Places) -> Result<()> {
    let base = places.daemon_dir().join("sizes");
    for (repo, folder) in entries_in(&base)? {
        for (name, record) in entries_in(&folder)? {
            let Some(checkout) = name.strip_suffix(".json") else {
                continue;
            };
            if !places
                .root
                .join(&repo)
                .join(checkout)
                .join(".checkout")
                .is_file()
            {
                std::fs::remove_file(&record)
                    .context(|| format!("removing {}", record.display()))?;
            }
        }
    }
    Ok(())
}

/// Measures each of <roots> and tells the dashboard about all of them.
pub fn record_roots(places: &Places, roots: &[PathBuf]) {
    if roots.is_empty() || !places.root.is_dir() {
        return;
    }
    for root in roots {
        if let Err(error) = record(places, root) {
            crate::text::note(format!(
                "sizes: could not measure {}: {}",
                root.display(),
                error.text()
            ));
            // The old record would show a size from before this command;
            // with none, the dashboard measures the root.
            if let Err(error) = remove(places, root) {
                crate::text::note(format!("sizes: {}", error.text()));
            }
        }
    }
    crate::daemon::client::wrote(places, roots);
}

/// The build roots `builds <command> <args>` writes in.
fn written_by(places: &Places, command: &str, args: &[String]) -> Result<Vec<PathBuf>> {
    let here = || -> Result<Vec<PathBuf>> {
        let dir =
            std::env::current_dir().context(|| "reading the working directory".to_string())?;
        Ok(crate::checkout::Checkout::at_keeping_env(&dir)?
            .map(|checkout| vec![checkout.build_root(places)])
            .unwrap_or_default())
    };
    match command {
        "cargo" | "lake" | "new" | "release" | "retire" => here(),
        "app-build" | "build-app" | "app-checkout" => {
            let mut roots = here()?;
            if let Some(repo) = args.iter().find(|arg| !arg.starts_with('-')) {
                for lane in crate::lane::Lane::ALL {
                    for entry in places.app_checkouts_of(repo, lane)? {
                        if !entry.contains(':') {
                            roots.push(places.root.join(repo).join(entry));
                        }
                    }
                }
            }
            roots.sort();
            roots.dedup();
            Ok(roots)
        }
        "prune" => Ok(crate::checkout::all_root_paths(places)?),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
#[path = "sizes_tests.rs"]
mod tests;
