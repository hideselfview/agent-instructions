//! The object files in a profile folder that no build or debugger reads.
//!
//! With split-debuginfo "unpacked", macOS's default, rustc leaves each unit's
//! object files in deps (<crate>-<hash>.<cgu>.<set>.rcgu.o, or
//! <crate>.<crate>.<set>-cgu.NN.rcgu.o for a library whose files carry no
//! hash), and a linked binary's debug map names the files it was linked from:
//! a test's or binary's own, the members of each rlib it links
//! (libtokio-<hash>.rlib(...)) rather than the rlib's loose objects, and for a
//! dylib or cdylib its own. Each build of an incremental unit, and each build
//! of an unhashed library, writes a new set under new names and leaves the old
//! ones, which no build and no debugger reads. rustc reuses an incremental
//! unit's objects from its incremental folder, never from deps.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use super::hashed_file;
use super::prune::Unit;
use crate::fail::Result;

const MH_MAGIC_64: u32 = 0xFEED_FACF;
const LC_SYMTAB: u32 = 0x2;
const N_OSO: u8 = 0x66;
const NLIST_64: usize = 16;

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_at(file: &std::fs::File, offset: u64, length: usize) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; length];
    file.read_exact_at(&mut buffer, offset).ok()?;
    Some(buffer)
}

/// The names of the loose object files the Mach-O binary <path> was linked
/// from, which its debug map names (N_OSO entries; an rlib's members,
/// libtokio-<hash>.rlib(...), aren't loose); None when <path> is no 64-bit
/// Mach-O this can read.
pub fn debug_map(path: &Path) -> Option<HashSet<String>> {
    let file = std::fs::File::open(path).ok()?;
    let header = read_at(&file, 0, 32)?;
    if u32_at(&header, 0)? != MH_MAGIC_64 {
        return None;
    }
    let commands = u32_at(&header, 16)? as usize;
    let commands_size = u32_at(&header, 20)? as usize;
    let loads = read_at(&file, 32, commands_size)?;
    let mut offset = 0;
    for _ in 0..commands {
        let command = u32_at(&loads, offset)?;
        let size = u32_at(&loads, offset + 4)? as usize;
        if command == LC_SYMTAB {
            let symbols = u32_at(&loads, offset + 8)? as u64;
            let count = u32_at(&loads, offset + 12)? as usize;
            let strings = u32_at(&loads, offset + 16)? as u64;
            let strings_size = u32_at(&loads, offset + 20)? as usize;
            let table = read_at(&file, symbols, count * NLIST_64)?;
            let text = read_at(&file, strings, strings_size)?;
            let mut names = HashSet::new();
            for entry in table.chunks_exact(NLIST_64) {
                if entry[4] != N_OSO {
                    continue;
                }
                let start = u32_at(entry, 0)? as usize;
                let rest = text.get(start..)?;
                let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
                let name = String::from_utf8_lossy(&rest[..end]);
                if !name.ends_with(')') {
                    let base = name.rsplit('/').next().unwrap_or("");
                    names.insert(base.to_string());
                }
            }
            return Some(names);
        }
        if size == 0 {
            return None;
        }
        offset += size;
    }
    Some(HashSet::new())
}

fn base(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

/// The token one build gives every object file of a unit it writes, the third
/// part of the name: <crate>-<hash>.<cgu>.<set>.rcgu.o (incremental),
/// <crate>-<hash>.<crate>.<set>-cgu.NN.rcgu.o,
/// <crate>.<crate>.<set>-cgu.NN.rcgu.o.
fn object_set(path: &Path) -> &str {
    base(path).split('.').nth(2).unwrap_or("")
}

/// Those of <objects> that none of <binaries>, the files linked from them,
/// names. One set is what the build that wrote it linked from, so only more
/// than one set is looked up; every set is unread without a binary. A binary
/// this cannot read keeps them all.
fn unread_objects(objects: Vec<PathBuf>, binaries: &[PathBuf]) -> Vec<PathBuf> {
    if binaries.is_empty() {
        return objects;
    }
    let sets: HashSet<&str> = objects.iter().map(|path| object_set(path)).collect();
    if sets.len() == 1 {
        return Vec::new();
    }
    let mut named = HashSet::new();
    for binary in binaries {
        match debug_map(binary) {
            Some(names) => named.extend(names),
            None => return Vec::new(),
        }
    }
    objects
        .into_iter()
        .filter(|path| !named.contains(base(path)))
        .collect()
}

/// The object files in <profile> that no build or debugger reads, among
/// <units>' and those of libraries whose files carry no hash: an rlib's
/// (binaries name the rlib's members), and any other unit's that its binary
/// or dylib doesn't name.
pub fn dead_objects(profile: &Path, units: &[&Unit]) -> Result<Vec<PathBuf>> {
    let mut dead = Vec::new();
    for found in units {
        let objects: Vec<PathBuf> = found
            .outputs
            .iter()
            .filter(|path| base(path).ends_with(".rcgu.o"))
            .cloned()
            .collect();
        if objects.is_empty() {
            continue;
        }
        let binaries: Vec<PathBuf> = found
            .outputs
            .iter()
            .filter(|path| base(path).ends_with(".dylib") || !base(path).contains('.'))
            .cloned()
            .collect();
        if binaries.is_empty()
            && found
                .outputs
                .iter()
                .any(|path| base(path).ends_with(".rlib"))
        {
            dead.extend(objects);
        } else {
            dead.extend(unread_objects(objects, &binaries));
        }
    }
    let deps = profile.join("deps");
    if deps.is_dir() {
        let mut unhashed: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut order = Vec::new();
        for name in super::names_in(&deps)? {
            if name.ends_with(".rcgu.o") && hashed_file(&name).is_none() {
                let krate = name.split('.').next().unwrap_or("").to_string();
                if !unhashed.contains_key(&krate) {
                    order.push(krate.clone());
                }
                unhashed.entry(krate).or_default().push(deps.join(&name));
            }
        }
        for krate in order {
            let objects = unhashed.remove(&krate).expect("listed above");
            let dylib = deps.join(format!("lib{krate}.dylib"));
            let binaries = if dylib.exists() {
                vec![dylib]
            } else {
                Vec::new()
            };
            dead.extend(unread_objects(objects, &binaries));
        }
    }
    Ok(dead)
}
