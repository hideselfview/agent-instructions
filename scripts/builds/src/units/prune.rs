//! Deleting the build copies no build used for days, the copies a newer copy
//! of the same thing replaced, the incremental folders only they used or
//! nothing wrote to for hours, and the object files nothing reads.

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::objects::dead_objects;
use super::record::{fingerprint_values, recorded};
use super::{
    INCREMENTAL_DEPTH, OUTPUT_DIRS, OpenFiles, hashed_file, holds_any, lock_profile, names_in, newest_mtime,
    profile_dirs, remove, unit_dir,
};
use crate::fail::Result;
use crate::text::{counted, human_bytes, note};
use crate::usage::{Seen, bytes_with, freeable};

/// The ages the rules go by, as cutoffs from one moment.
#[derive(Clone, Debug)]
pub struct Ages {
    pub days: u64,
    pub hours: u64,
    pub idle_hours: u64,
    /// No build used a unit since this: it goes.
    pub cutoff: f64,
    /// A replaced unit unused since this goes.
    pub since: f64,
    /// An incremental folder nothing wrote to since this goes.
    pub idle_cutoff: f64,
}

impl Ages {
    pub fn new(days: u64, hours: u64, idle_hours: u64) -> Ages {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_secs_f64();
        Ages {
            days,
            hours,
            idle_hours,
            cutoff: now - (days * 86400) as f64,
            since: now - (hours * 3600) as f64,
            idle_cutoff: now - (idle_hours * 3600) as f64,
        }
    }
}

/// One build copy: a crate target for one profile, target and feature set.
pub struct Unit {
    pub hash: String,
    pub name: Option<String>,
    pub fingerprint: Option<PathBuf>,
    pub build: Option<PathBuf>,
    pub outputs: Vec<PathBuf>,
    last_use: OnceCell<f64>,
}

impl Unit {
    fn new(hash: &str) -> Unit {
        Unit { hash: hash.to_string(), name: None, fingerprint: None, build: None, outputs: Vec::new(), last_use: OnceCell::new() }
    }

    /// The fingerprint goes first: without it cargo rebuilds the unit before
    /// anything reads its outputs, so a deletion cut off halfway leaves only
    /// files the unit's next build overwrites. Until then those files count
    /// as used when they were written (last_use).
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.fingerprint.iter().cloned().collect();
        paths.extend(self.outputs.iter().cloned());
        paths.extend(self.build.iter().cloned());
        paths
    }

    pub fn last_use(&self) -> Result<f64> {
        if let Some(when) = self.last_use.get() {
            return Ok(*when);
        }
        let when = match &self.fingerprint {
            Some(fingerprint) => newest_mtime(fingerprint, 1)?,
            None => {
                let mut newest = 0.0f64;
                for path in self.paths() {
                    newest = newest.max(newest_mtime(&path, 1)?);
                }
                newest
            }
        };
        Ok(*self.last_use.get_or_init(|| when))
    }
}

/// The units of one profile folder, in the order they were found.
#[derive(Default)]
pub struct Units {
    pub list: Vec<Unit>,
    index: HashMap<String, usize>,
}

impl Units {
    fn unit(&mut self, hash: &str) -> &mut Unit {
        let at = match self.index.get(hash) {
            Some(at) => *at,
            None => {
                self.list.push(Unit::new(hash));
                self.index.insert(hash.to_string(), self.list.len() - 1);
                self.list.len() - 1
            }
        };
        &mut self.list[at]
    }
}

/// Every unit in <profile>.
pub fn units_in(profile: &Path) -> Result<Units> {
    let mut units = Units::default();
    for name in names_in(&profile.join(".fingerprint"))? {
        if let Some((package, hash)) = unit_dir(&name) {
            let found = units.unit(hash);
            found.fingerprint = Some(profile.join(".fingerprint").join(&name));
            found.name = Some(package.to_string());
        }
    }
    let build = profile.join("build");
    if build.is_dir() {
        for name in names_in(&build)? {
            if let Some((package, hash)) = unit_dir(&name) {
                let found = units.unit(hash);
                found.build = Some(build.join(&name));
                found.name.get_or_insert_with(|| package.to_string());
            }
        }
    }
    for folder in OUTPUT_DIRS {
        let outputs = profile.join(folder);
        if !outputs.is_dir() {
            continue;
        }
        for name in names_in(&outputs)? {
            if let Some((krate, hash)) = hashed_file(&name) {
                let found = units.unit(hash);
                found.outputs.push(outputs.join(&name));
                found.name.get_or_insert_with(|| krate.to_string());
            }
        }
    }
    Ok(units)
}

/// Each folder in <profile>/incremental with the hashes of the units whose
/// deps files are hard links to its files (none: no unit's).
fn incremental_dirs(profile: &Path, units: &Units) -> Result<Vec<(PathBuf, BTreeSet<String>)>> {
    let incremental = profile.join("incremental");
    if !incremental.is_dir() {
        return Ok(Vec::new());
    }
    let crate_dirs: Vec<PathBuf> = names_in(&incremental)?
        .into_iter()
        .map(|name| incremental.join(name))
        .filter(|path| path.is_dir())
        .collect();
    if crate_dirs.is_empty() {
        return Ok(Vec::new());
    }
    let mut owner: HashMap<(u64, u64), String> = HashMap::new();
    for found in &units.list {
        for path in &found.outputs {
            if let Ok(meta) = std::fs::symlink_metadata(path)
                && meta.nlink() > 1 {
                    owner.insert((meta.dev(), meta.ino()), found.hash.clone());
                }
        }
    }
    let mut result = Vec::new();
    for crate_dir in crate_dirs {
        let owners = std::sync::Mutex::new(BTreeSet::new());
        let dev = std::fs::symlink_metadata(&crate_dir).map(|meta| meta.dev()).unwrap_or(0);
        crate::usage::walk(&crate_dir, dev, 1, &|_, meta| {
            if !meta.is_dir() && meta.nlink() > 1
                && let Some(hash) = owner.get(&(meta.dev(), meta.ino())) {
                    owners.lock().expect("no panic holding it").insert(hash.clone());
                }
        })?;
        result.push((crate_dir, owners.into_inner().expect("no panic holding it")));
    }
    Ok(result)
}

/// A unit across the profile folders one prune reads.
type Key = (usize, String);

/// Which units are builds of the same thing: the same package and target,
/// mode, features, profile settings, rustflags and target triple, in the same
/// profile folder, over dependencies that are themselves the same thing.
/// Copies with one identity differ only in their sources: a dependency's
/// version or git revision, or the compiler's version. Each identity is a
/// small number, shared by equal ones.
///
/// A dependent's fingerprint names each dependency by the dependency's
/// fingerprint when it was built. A workspace crate rebuilt in place since
/// (one some dependent names by a fingerprint no unit has now) counts by its
/// package and crate name alone, wherever it is named: its dependents rebuild
/// in place too on their next build. Units cargo puts in one fingerprint
/// folder (a library's check and its documentation) count as one.
struct Identities {
    unit_of: HashMap<u64, Key>,
    own: HashMap<Key, String>,
    /// [package id hash, crate name, fingerprint] per dependency.
    deps: HashMap<Key, Vec<(String, String, Option<u64>)>>,
    in_place: HashSet<(String, String)>,
    interned: HashMap<String, usize>,
    memo: HashMap<Key, Option<usize>>,
}

impl Identities {
    fn new(every: &[(PathBuf, Units)]) -> Result<Identities> {
        let mut unit_of = HashMap::new();
        let mut own = HashMap::new();
        let mut deps = HashMap::new();
        for (at, (profile, units)) in every.iter().enumerate() {
            for found in &units.list {
                let Some(fingerprint) = &found.fingerprint else { continue };
                let key = (at, found.hash.clone());
                let mut names: Vec<String> = match names_in(fingerprint) {
                    Ok(names) => names.into_iter().filter(|name| name.ends_with(".json")).collect(),
                    Err(_) if !fingerprint.exists() => continue,
                    Err(error) => return Err(error),
                };
                names.sort();
                let records: Vec<(String, Value)> = names
                    .iter()
                    .map(|name| (name.clone(), recorded(fingerprint, &name[..name.len() - ".json".len()])))
                    .collect();
                if records.is_empty() || !records.iter().all(|(_, record)| truthy(record)) {
                    continue;
                }
                let shape: Vec<Value> = records
                    .iter()
                    .map(|(name, record)| {
                        Value::Array(vec![
                            Value::String(name.clone()),
                            record.get("features").cloned().unwrap_or(Value::Null),
                            record.get("profile").cloned().unwrap_or(Value::Null),
                            record.get("compile_kind").cloned().unwrap_or(Value::Null),
                            Value::String(record.get("rustflags").cloned().unwrap_or(Value::Null).to_string()),
                            record.get("config").cloned().unwrap_or(Value::Null),
                        ])
                    })
                    .collect();
                own.insert(
                    key.clone(),
                    Value::Array(vec![
                        Value::String(profile.display().to_string()),
                        found.name.clone().map(Value::String).unwrap_or(Value::Null),
                        Value::Array(shape),
                    ])
                    .to_string(),
                );
                let mut listed = Vec::new();
                for (_, record) in &records {
                    for dep in record.get("deps").and_then(Value::as_array).into_iter().flatten() {
                        if let Value::Array(parts) = dep
                            && parts.len() >= 3 {
                                listed.push((
                                    parts[0].to_string(),
                                    parts[1].to_string(),
                                    parts[parts.len() - 1].as_u64(),
                                ));
                            }
                    }
                }
                deps.insert(key.clone(), listed);
                for value in fingerprint_values(fingerprint) {
                    unit_of.insert(value, key.clone());
                }
            }
        }
        let in_place = deps
            .values()
            .flatten()
            .filter(|(_, _, value)| value.is_none_or(|value| !unit_of.contains_key(&value)))
            .map(|(package, krate, _)| (package.clone(), krate.clone()))
            .collect();
        Ok(Identities { unit_of, own, deps, in_place, interned: HashMap::new(), memo: HashMap::new() })
    }

    /// The units <key> was built against that are still here.
    fn dependencies(&self, key: &Key) -> Vec<Key> {
        self.deps
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|(_, _, value)| value.and_then(|value| self.unit_of.get(&value)).cloned())
            .collect()
    }

    /// <key>'s identity, or None for a unit whose fingerprint, or a
    /// dependency's, can't be read.
    fn of(&mut self, key: &Key) -> Option<usize> {
        if let Some(known) = self.memo.get(key) {
            return *known;
        }
        self.memo.insert(key.clone(), None);
        if !self.own.contains_key(key) {
            return None;
        }
        let mut parts = Vec::new();
        for (package, krate, value) in self.deps[key].clone() {
            if self.in_place.contains(&(package.clone(), krate.clone())) {
                parts.push(Some(format!("name {package} {krate}")));
            } else {
                let dependency = self.unit_of[&value.expect("a dependency not in place has a known fingerprint")].clone();
                if dependency != *key {
                    parts.push(self.of(&dependency).map(|identity| format!("unit {identity}")));
                }
            }
        }
        let mut parts = parts.into_iter().collect::<Option<Vec<String>>>()?;
        parts.sort();
        let shape = format!("{}\n{}", self.own[key], parts.join("\n"));
        let next = self.interned.len();
        let identity = *self.interned.entry(shape).or_insert(next);
        self.memo.insert(key.clone(), Some(identity));
        Some(identity)
    }
}

/// A JSON value Python counts as true: a record that read as something.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Object(map) => !map.is_empty(),
        Value::Array(list) => !list.is_empty(),
        Value::String(text) => !text.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}

/// The units in the profile folders <free> of <every> that another copy with
/// the same identity replaced: one used after them, when nothing used them
/// since <since> and no unit that stays was built against them. A unit there
/// unused since <cutoff> goes anyway and keeps nothing.
fn superseded(every: &[(PathBuf, Units)], free: &HashSet<usize>, since: f64, cutoff: f64) -> Result<HashSet<Key>> {
    let mut identities = Identities::new(every)?;
    let mut groups: HashMap<usize, Vec<(Key, f64)>> = HashMap::new();
    for (at, (_, units)) in every.iter().enumerate() {
        for found in &units.list {
            let key = (at, found.hash.clone());
            if let Some(identity) = identities.of(&key) {
                groups.entry(identity).or_default().push((key, found.last_use()?));
            }
        }
    }
    let mut replaced = HashSet::new();
    for members in groups.values() {
        let newest = members.iter().map(|(_, used)| *used).fold(f64::MIN, f64::max);
        for (key, used) in members {
            if free.contains(&key.0) && *used < newest && *used < since {
                replaced.insert(key.clone());
            }
        }
    }
    // Two versions of one crate in a single build look alike; a unit that
    // stays keeps what it was built against.
    let mut pending = Vec::new();
    for (at, (_, units)) in every.iter().enumerate() {
        for found in &units.list {
            let key = (at, found.hash.clone());
            if !replaced.contains(&key) && (!free.contains(&at) || found.last_use()? >= cutoff) {
                pending.push(key);
            }
        }
    }
    let mut seen: HashSet<Key> = pending.iter().cloned().collect();
    while let Some(key) = pending.pop() {
        for dependency in identities.dependencies(&key) {
            replaced.remove(&dependency);
            if seen.insert(dependency.clone()) {
                pending.push(dependency);
            }
        }
    }
    Ok(replaced)
}

fn single_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.nlink() == 1)
}

fn verb(dry: bool) -> &'static str {
    if dry { "would delete" } else { "deleted" }
}

/// Deletes <profile>'s <units> no build used since the cutoff, those in
/// <replaced> (superseded), the incremental folders only they used, every
/// incremental folder nothing wrote anywhere inside since the idle cutoff,
/// and the other units' dead object files, but none a process holds a file
/// of. Returns the bytes freed (or that would be).
fn prune_profile(
    profile: &Path,
    units: &Units,
    replaced: &HashSet<String>,
    ages: &Ages,
    dry: bool,
    open_files: &mut OpenFiles,
) -> Result<u64> {
    let mut stale: Vec<&Unit> = Vec::new();
    for found in &units.list {
        if found.last_use()? < ages.cutoff {
            stale.push(found);
        }
    }
    let stale_hashes: HashSet<&str> = stale.iter().map(|u| u.hash.as_str()).collect();
    let mut older: Vec<&Unit> = units
        .list
        .iter()
        .filter(|u| replaced.contains(&u.hash) && !stale_hashes.contains(u.hash.as_str()))
        .collect();
    let every_increment = incremental_dirs(profile, units)?;
    let going = |stale: &[&Unit], older: &[&Unit], owners: &BTreeSet<String>| {
        owners.iter().all(|hash| stale.iter().chain(older).any(|u| &u.hash == hash))
    };
    let mut increments = Vec::new();
    for (crate_dir, owners) in &every_increment {
        if (!owners.is_empty() && going(&stale, &older, owners))
            || (owners.is_empty() && newest_mtime(crate_dir, 2)? < ages.cutoff)
        {
            increments.push((crate_dir.clone(), owners.clone()));
        }
    }
    let taken: HashSet<&PathBuf> = increments.iter().map(|(dir, _)| dir).collect();
    let mut idle = Vec::new();
    for (crate_dir, _) in &every_increment {
        if !taken.contains(crate_dir) && newest_mtime(crate_dir, INCREMENTAL_DEPTH)? < ages.idle_cutoff {
            idle.push(crate_dir.clone());
        }
    }
    let remaining: Vec<&Unit> = units
        .list
        .iter()
        .filter(|u| !stale_hashes.contains(u.hash.as_str()) && !older.iter().any(|o| o.hash == u.hash))
        .collect();
    let mut objects = dead_objects(profile, &remaining)?;
    if stale.is_empty() && older.is_empty() && increments.is_empty() && idle.is_empty() && objects.is_empty() {
        return Ok(0);
    }
    let held = open_files.get()?.clone();
    let mut kept = Vec::new();
    for found in stale.iter().chain(older.iter()) {
        if holds_any(&found.paths(), &held)? {
            kept.push(found.hash.clone());
        }
    }
    stale.retain(|u| !kept.contains(&u.hash));
    older.retain(|u| !kept.contains(&u.hash));
    let mut increment_dirs = Vec::new();
    for (crate_dir, owners) in increments {
        if going(&stale, &older, &owners) && !holds_any(std::slice::from_ref(&crate_dir), &held)? {
            increment_dirs.push(crate_dir);
        }
    }
    let mut idle_dirs = Vec::new();
    for crate_dir in idle {
        if !holds_any(std::slice::from_ref(&crate_dir), &held)? {
            idle_dirs.push(crate_dir);
        }
    }
    let mut unheld = Vec::new();
    for path in objects.drain(..) {
        if !holds_any(std::slice::from_ref(&path), &held)? {
            unheld.push(path);
        }
    }
    let objects = unheld;
    let seen = Seen::default();
    let mut freed = 0;
    let groups = [
        (&stale, format!("no build used in {} days", ages.days)),
        (&older, format!("a newer copy replaced, unused for {} hours", ages.hours)),
    ];
    for (group, why) in groups {
        if group.is_empty() {
            continue;
        }
        let mut size = 0;
        let mut by_name: Vec<(String, usize, u64)> = Vec::new();
        for found in group.iter() {
            let mut unit_size = 0;
            for path in found.paths() {
                unit_size += bytes_with(&seen, &path)?;
            }
            size += unit_size;
            let name = found.name.clone().unwrap_or_default();
            match by_name.iter_mut().find(|(n, _, _)| *n == name) {
                Some(entry) => {
                    entry.1 += 1;
                    entry.2 += unit_size;
                }
                None => by_name.push((name, 1, unit_size)),
            }
            if !dry {
                for path in found.paths() {
                    remove(&path)?;
                }
            }
        }
        freed += size;
        by_name.sort_by_key(|entry| std::cmp::Reverse(entry.2));
        let most: Vec<String> = by_name
            .iter()
            .take(3)
            .map(|(name, count, total)| format!("{name} x{count} {}", human_bytes(*total)))
            .collect();
        note(format!(
            "{} {} {why} from {} ({}; most: {})",
            verb(dry),
            counted(group.len(), "build copy", "build copies"),
            profile.display(),
            human_bytes(size),
            most.join(", ")
        ));
    }
    if !increment_dirs.is_empty() {
        let mut size = 0;
        for crate_dir in &increment_dirs {
            size += bytes_with(&seen, crate_dir)?;
        }
        freed += size;
        if !dry {
            for crate_dir in &increment_dirs {
                remove(crate_dir)?;
            }
        }
        note(format!(
            "{} {} no build copy left uses from {} ({})",
            verb(dry),
            counted(increment_dirs.len(), "incremental folder", "incremental folders"),
            profile.display(),
            human_bytes(size)
        ));
    }
    if !idle_dirs.is_empty() {
        // Not what deleting frees: rustc hard-links a unit's deps files into
        // these folders, and those stay with the unit.
        let mut sizes = Vec::new();
        for crate_dir in &idle_dirs {
            sizes.push(freeable(crate_dir)?);
        }
        let size: u64 = sizes.iter().sum();
        freed += size;
        if !dry {
            for crate_dir in &idle_dirs {
                remove(crate_dir)?;
            }
        }
        let mut order: Vec<usize> = (0..idle_dirs.len()).collect();
        order.sort_by(|a, b| sizes[*b].cmp(&sizes[*a]));
        let most: Vec<String> = order
            .iter()
            .take(3)
            .map(|at| {
                let name = idle_dirs[*at].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                format!("{name} {}", human_bytes(sizes[*at]))
            })
            .collect();
        note(format!(
            "{} {} nothing wrote to in {} hours from {} ({}; most: {})",
            verb(dry),
            counted(idle_dirs.len(), "incremental folder", "incremental folders"),
            ages.idle_hours,
            profile.display(),
            human_bytes(size),
            most.join(", ")
        ));
    }
    if !objects.is_empty() {
        // A file the incremental folder also links stays on disk.
        let mut size = 0;
        for path in &objects {
            if single_link(path) {
                size += bytes_with(&seen, path)?;
            }
        }
        freed += size;
        if !dry {
            for path in &objects {
                remove(path)?;
            }
        }
        note(format!(
            "{} {} no build or debugger reads from {} ({})",
            verb(dry),
            counted(objects.len(), "object file", "object files"),
            profile.display(),
            human_bytes(size)
        ));
    }
    if !kept.is_empty() {
        note(format!(
            "kept {} in {}: a process holds their files",
            counted(kept.len(), "build copy", "build copies"),
            profile.display()
        ));
    }
    Ok(freed)
}

/// Prunes each build folder's profile folders, each while holding its cargo
/// lock (a dry run takes none and deletes nothing). Returns the KiB freed per
/// folder; notes go to stderr.
pub fn prune(ages: &Ages, dry: bool, folders: &[PathBuf]) -> Result<Vec<(PathBuf, u64)>> {
    let mut result = Vec::new();
    for folder in folders {
        let root = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.clone());
        let profiles = profile_dirs(&root);
        let mut locks = Vec::new();
        let mut free = HashSet::new();
        for (at, profile) in profiles.iter().enumerate() {
            if dry {
                free.insert(at);
            } else if let Some(lock) = lock_profile(profile)? {
                locks.push(lock);
                free.insert(at);
            }
        }
        // Read without the locks of folders a build holds, only to keep what
        // they were built against and to say what waits.
        let mut every = Vec::new();
        for profile in &profiles {
            every.push((profile.clone(), units_in(profile)?));
        }
        let replaced = superseded(&every, &free, ages.since, ages.cutoff)?;
        for (at, (profile, units)) in every.iter().enumerate() {
            if free.contains(&at) {
                continue;
            }
            let mut waiting = 0;
            for found in &units.list {
                if found.last_use()? < ages.cutoff {
                    waiting += 1;
                }
            }
            if waiting > 0 {
                note(format!(
                    "kept {} in {}: a build holds its lock",
                    counted(waiting, "unused build copy", "unused build copies"),
                    profile.display()
                ));
            }
            let incremental = profile.join("incremental");
            let mut idle = 0;
            if incremental.is_dir() {
                for name in names_in(&incremental)? {
                    if newest_mtime(&incremental.join(name), INCREMENTAL_DEPTH)? < ages.idle_cutoff {
                        idle += 1;
                    }
                }
            }
            if idle > 0 {
                note(format!(
                    "kept {} nothing wrote to in {} hours in {}: a build holds its lock",
                    counted(idle, "incremental folder", "incremental folders"),
                    ages.idle_hours,
                    profile.display()
                ));
            }
        }
        let mut open_files = OpenFiles::new(std::slice::from_ref(&root));
        let mut freed = 0;
        for (at, (profile, units)) in every.iter().enumerate() {
            if !free.contains(&at) {
                continue;
            }
            let replaced_here: HashSet<String> =
                replaced.iter().filter(|(p, _)| *p == at).map(|(_, hash)| hash.clone()).collect();
            freed += prune_profile(profile, units, &replaced_here, ages, dry, &mut open_files)?;
        }
        drop(locks);
        result.push((folder.clone(), freed / 1024));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truthiness() {
        assert!(!truthy(&Value::Object(Default::default())));
        assert!(truthy(&serde_json::json!({"a": 1})));
    }
}
