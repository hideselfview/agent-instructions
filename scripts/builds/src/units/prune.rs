//! Deleting the build copies no build used for days, the copies a newer copy
//! of the same thing replaced, the incremental folders only they used or
//! nothing wrote to for hours (every one, in a checkout that builds without
//! incremental compilation), and the object files nothing reads.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::Value;

use super::objects::dead_objects;
use super::record::{fingerprint_values, recorded};
use super::{
    Guard, INCREMENTAL_DEPTH, OUTPUT_DIRS, OpenFiles, hashed_file, holds_any, lock_guard,
    lock_profile, names_in, newest_mtime, profile_dirs, remove, unit_dir,
};
use crate::fail::Result;
use crate::text::{counted, human_bytes, note};
use crate::usage::{Seen, bytes_with, freeable};

/// The ages the rules go by, as cutoffs from one moment.
#[derive(Clone, Debug)]
pub struct Ages {
    pub days: u64,
    /// 0 for a checkout whose builds run without incremental compilation,
    /// where no build reads an incremental folder.
    pub idle_hours: u64,
    /// No build used a unit since this: it goes.
    pub cutoff: f64,
    /// An incremental folder nothing wrote to since this goes.
    pub idle_cutoff: f64,
}

impl Ages {
    pub fn new(days: u64, idle_hours: u64) -> Ages {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_secs_f64();
        Ages {
            days,
            idle_hours,
            cutoff: now - (days * 86400) as f64,
            idle_cutoff: if idle_hours == 0 {
                f64::INFINITY
            } else {
                now - (idle_hours * 3600) as f64
            },
        }
    }

    /// Why an incremental folder the idle rule picks goes.
    fn idle_why(&self) -> String {
        if self.idle_hours == 0 {
            "this checkout builds without incremental compilation".to_string()
        } else {
            format!("nothing wrote to in {} hours", self.idle_hours)
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
        Unit {
            hash: hash.to_string(),
            name: None,
            fingerprint: None,
            build: None,
            outputs: Vec::new(),
            last_use: OnceCell::new(),
        }
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

    /// When the last build that used it began: the fingerprint folder's own
    /// modification time, which the recorder sets to the moment that build's
    /// cargo began reporting, the same for every unit of one build. Files
    /// cargo writes in the folder are newer, so for a unit a build rebuilt
    /// this is older than its last use. A unit no recorded build used has
    /// the time cargo last added a file to the folder.
    fn began(&self) -> Result<f64> {
        let Some(fingerprint) = &self.fingerprint else {
            return self.last_use();
        };
        newest_mtime(fingerprint, 0)
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
                && meta.nlink() > 1
            {
                owner.insert((meta.dev(), meta.ino()), found.hash.clone());
            }
        }
    }
    let mut result = Vec::new();
    for crate_dir in crate_dirs {
        let owners = std::sync::Mutex::new(BTreeSet::new());
        let dev = std::fs::symlink_metadata(&crate_dir)
            .map(|meta| meta.dev())
            .unwrap_or(0);
        crate::usage::walk(&crate_dir, dev, 1, &|_, meta| {
            if !meta.is_dir()
                && meta.nlink() > 1
                && let Some(hash) = owner.get(&(meta.dev(), meta.ino()))
            {
                owners
                    .lock()
                    .expect("no panic holding it")
                    .insert(hash.clone());
            }
        })?;
        result.push((crate_dir, owners.into_inner().expect("no panic holding it")));
    }
    Ok(result)
}

/// A unit across the profile folders one prune reads.
type Key = (usize, String);

/// The sources a unit and everything it was built against came from: the
/// compiler (cargo's hash of `rustc -vV`); each package's source folder
/// (cargo's hash of its path: a registry version's folder, a git revision's
/// checkout, a workspace member's own folder), by package; and the units
/// themselves, by identity (`Copies::identity`), which tells a workspace
/// crate's new version from its old one in the same folder.
#[derive(Default)]
struct Sources {
    rustc: BTreeSet<u64>,
    paths: BTreeMap<usize, BTreeSet<u64>>,
    units: BTreeMap<usize, BTreeSet<usize>>,
}

impl Sources {
    /// Were the two built from other sources: another compiler, another
    /// source folder of a package both were built against, or another copy
    /// of a unit both were built against a copy of? Copies built from the
    /// same sources share every unit of an identity, since cargo names a unit
    /// by its identity and sources.
    fn conflict(&self, other: &Sources) -> bool {
        fn differ<K: Ord, V: Eq>(mine: &BTreeMap<K, V>, theirs: &BTreeMap<K, V>) -> bool {
            mine.iter()
                .any(|(key, values)| theirs.get(key).is_some_and(|other| other != values))
        }
        self.rustc != other.rustc
            || differ(&self.paths, &other.paths)
            || differ(&self.units, &other.units)
    }

    fn extend(&mut self, other: &Sources) {
        self.rustc.extend(other.rustc.iter().copied());
        for (package, paths) in &other.paths {
            self.paths
                .entry(*package)
                .or_default()
                .extend(paths.iter().copied());
        }
        for (identity, units) in &other.units {
            self.units
                .entry(*identity)
                .or_default()
                .extend(units.iter().copied());
        }
    }
}

/// Which units are copies of the same thing, and what each was built from
/// and against, read from the fingerprint JSON of every unit that has one.
///
/// Copies of one thing are builds of the same package and target, mode,
/// features, profile settings, rustflags and target triple, in the same
/// profile folder (`thing`). A newer copy replaced an older one when it was
/// built from other sources (`Sources::conflict`): a dependency's version or
/// git revision, or the compiler, which can also change a dependency's
/// features (a new coven asking zeroize for another feature). Copies built
/// from the same sources differ in their dependencies' features, which the
/// package selection decides (`-p x` against the workspace); those are kept
/// side by side, as builds use them in turn. Units cargo puts in one
/// fingerprint folder (a library's check and its documentation) count as
/// one.
///
/// A unit's identity is its thing over the identities of what it was built
/// against: copies with one identity differ only in their sources. A
/// dependent's fingerprint names each dependency by the dependency's
/// fingerprint when it was built. A workspace crate rebuilt in place since
/// (one some dependent names by a fingerprint no unit has now) counts by its
/// package and crate name alone, wherever it is named: its dependents rebuild
/// in place too on their next build.
struct Copies {
    /// Each fingerprint value cargo records, by the unit it is of: a
    /// dependent's JSON names each dependency by the value it had when the
    /// dependent was built.
    unit_of: HashMap<u64, Key>,
    thing: HashMap<Key, String>,
    own_sources: HashMap<Key, (usize, Vec<u64>, Vec<u64>)>,
    /// [package id hash, crate name, fingerprint] per dependency.
    deps: HashMap<Key, Vec<(String, String, Option<u64>)>>,
    in_place: HashSet<(String, String)>,
    packages: HashMap<String, usize>,
    numbers: HashMap<Key, usize>,
    identities: HashMap<String, usize>,
    identity: HashMap<Key, usize>,
    sources: HashMap<Key, Rc<Sources>>,
}

impl Copies {
    fn new(every: &[(PathBuf, Units)]) -> Result<Copies> {
        let mut copies = Copies {
            unit_of: HashMap::new(),
            thing: HashMap::new(),
            own_sources: HashMap::new(),
            deps: HashMap::new(),
            in_place: HashSet::new(),
            packages: HashMap::new(),
            numbers: HashMap::new(),
            identities: HashMap::new(),
            identity: HashMap::new(),
            sources: HashMap::new(),
        };
        for (at, (profile, units)) in every.iter().enumerate() {
            for found in &units.list {
                let Some(fingerprint) = &found.fingerprint else {
                    continue;
                };
                let key = (at, found.hash.clone());
                let mut names: Vec<String> = match names_in(fingerprint) {
                    Ok(names) => names
                        .into_iter()
                        .filter(|name| name.ends_with(".json"))
                        .collect(),
                    Err(_) if !fingerprint.exists() => continue,
                    Err(error) => return Err(error),
                };
                names.sort();
                let records: Vec<(String, Value)> = names
                    .iter()
                    .map(|name| {
                        (
                            name.clone(),
                            recorded(fingerprint, &name[..name.len() - ".json".len()]),
                        )
                    })
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
                            Value::String(
                                record
                                    .get("rustflags")
                                    .cloned()
                                    .unwrap_or(Value::Null)
                                    .to_string(),
                            ),
                            record.get("config").cloned().unwrap_or(Value::Null),
                        ])
                    })
                    .collect();
                let package = found.name.clone().unwrap_or_default();
                copies.thing.insert(
                    key.clone(),
                    Value::Array(vec![
                        Value::String(profile.display().to_string()),
                        Value::String(package.clone()),
                        Value::Array(shape),
                    ])
                    .to_string(),
                );
                let next = copies.packages.len();
                let package = *copies.packages.entry(package).or_insert(next);
                let field = |name: &str| -> Vec<u64> {
                    records
                        .iter()
                        .filter_map(|(_, record)| record.get(name).and_then(Value::as_u64))
                        .collect()
                };
                copies
                    .own_sources
                    .insert(key.clone(), (package, field("path"), field("rustc")));
                let mut listed = Vec::new();
                for (_, record) in &records {
                    for dep in record
                        .get("deps")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Value::Array(parts) = dep
                            && parts.len() >= 3
                        {
                            listed.push((
                                parts[0].to_string(),
                                parts[1].to_string(),
                                parts[parts.len() - 1].as_u64(),
                            ));
                        }
                    }
                }
                copies.deps.insert(key.clone(), listed);
                for value in fingerprint_values(fingerprint) {
                    copies.unit_of.insert(value, key.clone());
                }
            }
        }
        copies.in_place = copies
            .deps
            .values()
            .flatten()
            .filter(|(_, _, value)| value.is_none_or(|value| !copies.unit_of.contains_key(&value)))
            .map(|(package, krate, _)| (package.clone(), krate.clone()))
            .collect();
        Ok(copies)
    }

    /// <key>'s identity, a small number shared by equal ones.
    fn identity(&mut self, key: &Key) -> usize {
        if let Some(known) = self.identity.get(key) {
            return *known;
        }
        // A placeholder, should the fingerprints ever name a cycle.
        self.identity.insert(key.clone(), usize::MAX);
        let mut parts = Vec::new();
        for (package, krate, value) in self.deps.get(key).cloned().unwrap_or_default() {
            let dependency = value.and_then(|value| self.unit_of.get(&value)).cloned();
            match dependency {
                Some(dependency) if !self.in_place.contains(&(package.clone(), krate.clone())) => {
                    if dependency != *key {
                        parts.push(format!("unit {}", self.identity(&dependency)));
                    }
                }
                _ => parts.push(format!("name {package} {krate}")),
            }
        }
        parts.sort();
        let shape = format!(
            "{}\n{}",
            self.thing.get(key).map(String::as_str).unwrap_or(""),
            parts.join("\n")
        );
        let next = self.identities.len();
        let identity = *self.identities.entry(shape).or_insert(next);
        self.identity.insert(key.clone(), identity);
        identity
    }

    /// The units <key> was built against that are still here. A workspace
    /// crate rebuilt in place since is named by a fingerprint no unit has
    /// now; its dependents rebuild on their next build.
    fn dependencies(&self, key: &Key) -> Vec<Key> {
        self.deps
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|(_, _, value)| value.and_then(|value| self.unit_of.get(&value)))
            .filter(|dependency| *dependency != key)
            .cloned()
            .collect()
    }

    /// The sources of <key> and of everything it was built against that is
    /// still here.
    fn sources_of(&mut self, key: &Key) -> Rc<Sources> {
        if let Some(known) = self.sources.get(key) {
            return known.clone();
        }
        // A placeholder, should the fingerprints ever name a cycle.
        self.sources
            .insert(key.clone(), Rc::new(Sources::default()));
        let mut sources = Sources::default();
        if let Some((package, paths, rustc)) = self.own_sources.get(key).cloned() {
            sources.rustc.extend(rustc);
            sources.paths.entry(package).or_default().extend(paths);
        }
        let identity = self.identity(key);
        let next = self.numbers.len();
        let number = *self.numbers.entry(key.clone()).or_insert(next);
        sources.units.entry(identity).or_default().insert(number);
        for dependency in self.dependencies(key) {
            let theirs = self.sources_of(&dependency);
            sources.extend(&theirs);
        }
        let sources = Rc::new(sources);
        self.sources.insert(key.clone(), sources.clone());
        sources
    }

    /// Did one of <members>, copies of <key>'s thing, replace it: built from
    /// other sources and used by a build that began after <key>'s last use?
    fn replaced_by_any(
        &mut self,
        key: &Key,
        members: &[Key],
        times: &HashMap<Key, (f64, f64)>,
    ) -> bool {
        let used = times[key].0;
        let mine = self.sources_of(key);
        members
            .iter()
            .any(|other| times[other].1 > used && self.sources_of(other).conflict(&mine))
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

/// The units in the profile folders <free> of <every> that are out of date,
/// when no unit that stays was built against them: another copy of the same
/// thing, built from other sources, was used by a build that began after
/// their last use; or they were built against a unit that a unit of their
/// package used by such a build was built against another copy of (a test
/// binary no build rebuilt since the library beside it moved to a new
/// dependency). A unit there unused since <cutoff> goes anyway and keeps
/// nothing.
///
/// Two versions of one crate in one build (thiserror 1 and 2) look like two
/// copies of one thing built from other sources, but that build began before
/// either's last use, so neither replaced the other; and a unit that stays
/// keeps both.
fn superseded(
    every: &[(PathBuf, Units)],
    free: &HashSet<usize>,
    cutoff: f64,
) -> Result<HashSet<Key>> {
    let mut copies = Copies::new(every)?;
    // Each copy's last use, and when the last build that used it began.
    let mut times: HashMap<Key, (f64, f64)> = HashMap::new();
    let mut groups: HashMap<String, Vec<Key>> = HashMap::new();
    let mut by_package: HashMap<usize, Vec<Key>> = HashMap::new();
    for (at, (_, units)) in every.iter().enumerate() {
        for found in &units.list {
            let key = (at, found.hash.clone());
            if let Some(thing) = copies.thing.get(&key) {
                times.insert(key.clone(), (found.last_use()?, found.began()?));
                groups.entry(thing.clone()).or_default().push(key.clone());
                by_package
                    .entry(copies.own_sources[&key].0)
                    .or_default()
                    .push(key);
            }
        }
    }
    let mut replaced = HashSet::new();
    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        for key in members {
            if free.contains(&key.0) && copies.replaced_by_any(key, members, &times) {
                replaced.insert(key.clone());
            }
        }
    }
    // A unit whose package a later build used against another copy, from
    // other sources, of something the unit was built against.
    for (key, (used, _)) in &times {
        if !free.contains(&key.0) || replaced.contains(key) {
            continue;
        }
        let mine = copies.dependencies(key);
        let package = copies.own_sources[key].0;
        let newer: Vec<&Key> = by_package[&package]
            .iter()
            .filter(|other| times[*other].1 > *used)
            .collect();
        'own: for other in newer {
            let theirs = copies.dependencies(other);
            for dependency in &mine {
                if theirs.contains(dependency) {
                    continue;
                }
                let Some(thing) = copies.thing.get(dependency).cloned() else {
                    continue;
                };
                let moved: Vec<Key> = theirs
                    .iter()
                    .filter(|theirs| copies.thing.get(*theirs) == Some(&thing))
                    .cloned()
                    .collect();
                let was = copies.sources_of(dependency);
                for now in moved {
                    if copies.sources_of(&now).conflict(&was) {
                        replaced.insert(key.clone());
                        break 'own;
                    }
                }
            }
        }
    }
    // A unit that stays keeps what it was built against.
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
        for dependency in copies.dependencies(&key) {
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
        owners
            .iter()
            .all(|hash| stale.iter().chain(older).any(|u| &u.hash == hash))
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
        if !taken.contains(crate_dir)
            && newest_mtime(crate_dir, INCREMENTAL_DEPTH)? < ages.idle_cutoff
        {
            idle.push(crate_dir.clone());
        }
    }
    let remaining: Vec<&Unit> = units
        .list
        .iter()
        .filter(|u| {
            !stale_hashes.contains(u.hash.as_str()) && !older.iter().any(|o| o.hash == u.hash)
        })
        .collect();
    let mut objects = dead_objects(profile, &remaining)?;
    if stale.is_empty()
        && older.is_empty()
        && increments.is_empty()
        && idle.is_empty()
        && objects.is_empty()
    {
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
        (
            &older,
            "a later build from other sources replaced".to_string(),
        ),
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
            counted(
                increment_dirs.len(),
                "incremental folder",
                "incremental folders"
            ),
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
                let name = idle_dirs[*at]
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{name} {}", human_bytes(sizes[*at]))
            })
            .collect();
        note(format!(
            "{} {} {} from {} ({}; most: {})",
            verb(dry),
            counted(idle_dirs.len(), "incremental folder", "incremental folders"),
            ages.idle_why(),
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

/// Prunes each build folder's profile folders, while holding its checkout's
/// guard and each profile folder's cargo lock (a dry run takes none and
/// deletes nothing). A folder whose guard a `builds cargo` build holds is
/// left whole; a profile folder whose lock a build holds keeps its units.
/// Returns the KiB freed per folder; notes go to stderr.
pub fn prune(ages: &Ages, dry: bool, folders: &[PathBuf]) -> Result<Vec<(PathBuf, u64)>> {
    let mut result = Vec::new();
    for folder in folders {
        let root = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.clone());
        // Held until this folder's units are done with.
        let guard = if dry {
            Guard::Absent
        } else {
            lock_guard(&root)?
        };
        if let Guard::Busy = guard {
            note(format!(
                "kept the build copies in {}: a build is running in its checkout",
                folder.display()
            ));
            result.push((folder.clone(), 0));
            continue;
        }
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
        let replaced = superseded(&every, &free, ages.cutoff)?;
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
                    if newest_mtime(&incremental.join(name), INCREMENTAL_DEPTH)? < ages.idle_cutoff
                    {
                        idle += 1;
                    }
                }
            }
            if idle > 0 {
                note(format!(
                    "kept {} in {} ({}): a build holds its lock",
                    counted(idle, "incremental folder", "incremental folders"),
                    profile.display(),
                    ages.idle_why()
                ));
            }
        }
        let mut open_files = OpenFiles::new(std::slice::from_ref(&root));
        let mut freed = 0;
        for (at, (profile, units)) in every.iter().enumerate() {
            if !free.contains(&at) {
                continue;
            }
            let replaced_here: HashSet<String> = replaced
                .iter()
                .filter(|(p, _)| *p == at)
                .map(|(_, hash)| hash.clone())
                .collect();
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
