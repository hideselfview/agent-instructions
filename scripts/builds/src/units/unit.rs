//! The units of a profile folder, in either of cargo's layouts, and the
//! paths each one's files are at.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    OUTPUT_DIRS, has_shared_folders, hashed_file, is_file, names_if_present, names_in,
    newest_mtime, remove, stat, unit_dir, unit_folders,
};
use crate::fail::Result;

/// One build copy: a crate target for one profile, target and feature set.
pub struct Unit {
    pub hash: String,
    pub name: Option<String>,
    pub fingerprint: Option<PathBuf>,
    /// The files (never folders) rustc wrote for it, which its binaries name and its
    /// incremental folder hard-links: in deps or examples (shared folders) or
    /// in its out folder (unit folders).
    pub outputs: Vec<PathBuf>,
    place: Place,
    last_use: OnceCell<f64>,
}

/// Where a unit's files are, besides its fingerprint folder.
enum Place {
    /// Shared folders: its outputs, and build/<package>-<hash>/ for a build
    /// script's build or run.
    Shared { build: Option<PathBuf> },
    /// Unit folders: its own folder, build/<package>/<hash>/, with every
    /// entry in it but the fingerprint folder.
    Own {
        folder: PathBuf,
        entries: Vec<PathBuf>,
    },
}

impl Unit {
    fn new(hash: &str) -> Unit {
        Unit {
            hash: hash.to_string(),
            name: None,
            fingerprint: None,
            outputs: Vec::new(),
            place: Place::Shared { build: None },
            last_use: OnceCell::new(),
        }
    }

    /// The fingerprint goes first: without it cargo rebuilds the unit before
    /// anything reads its outputs, so a deletion cut off halfway leaves only
    /// files the unit's next build overwrites. Until then those files count
    /// as used when they were written (last_use). The paths don't overlap,
    /// so their sizes add up to the unit's.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.fingerprint.iter().cloned().collect();
        match &self.place {
            Place::Shared { build } => {
                paths.extend(self.outputs.iter().cloned());
                paths.extend(build.iter().cloned());
            }
            Place::Own { entries, .. } => paths.extend(entries.iter().cloned()),
        }
        paths
    }

    /// Deletes its paths in order, then its own folder, which is empty by
    /// then.
    pub fn delete(&self) -> Result<()> {
        for path in self.paths() {
            remove(&path)?;
        }
        if let Place::Own { folder, .. } = &self.place {
            remove(folder)?;
        }
        Ok(())
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
    pub fn began(&self) -> Result<f64> {
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

/// Every unit in <profile>, in either layout.
pub fn units_in(profile: &Path) -> Result<Units> {
    let mut units = Units::default();
    if has_shared_folders(profile)? {
        for name in names_in(&profile.join(".fingerprint"))? {
            if let Some((package, hash)) = unit_dir(&name) {
                let found = units.unit(hash);
                found.fingerprint = Some(profile.join(".fingerprint").join(&name));
                found.name = Some(package.to_string());
            }
        }
    }
    let build = profile.join("build");
    for name in names_if_present(&build)? {
        let path = build.join(&name);
        let folders = unit_folders(&path)?;
        if folders.is_empty() {
            if let Some((package, hash)) = unit_dir(&name) {
                let found = units.unit(hash);
                found.place = Place::Shared {
                    build: Some(path.clone()),
                };
                found.name.get_or_insert_with(|| package.to_string());
            }
            continue;
        }
        for (hash, folder) in folders {
            let found = units.unit(&hash);
            found.name = Some(name.clone());
            let mut entries = Vec::new();
            for entry in names_in(&folder)? {
                if entry == "fingerprint" {
                    found.fingerprint = Some(folder.join(entry));
                } else {
                    entries.push(folder.join(entry));
                }
            }
            // A build script's run has a run folder, and its out folder
            // holds what the script wrote, not rustc.
            let out = folder.join("out");
            if stat(&folder.join("run"))?.is_none() {
                for file in names_if_present(&out)? {
                    if is_file(&out.join(&file))? {
                        found.outputs.push(out.join(file));
                    }
                }
            }
            found.place = Place::Own { folder, entries };
        }
    }
    if has_shared_folders(profile)? {
        for folder in OUTPUT_DIRS {
            let outputs = profile.join(folder);
            for name in names_if_present(&outputs)? {
                if let Some((krate, hash)) = hashed_file(&name)
                    && is_file(&outputs.join(&name))?
                {
                    let found = units.unit(hash);
                    found.outputs.push(outputs.join(&name));
                    found.name.get_or_insert_with(|| krate.to_string());
                }
            }
        }
    }
    Ok(units)
}

/// How many of <profile>'s units, as `units_in` lists them, have a
/// fingerprint folder: the units cargo built there.
pub fn built_count(profile: &Path) -> Result<usize> {
    Ok(units_in(profile)?
        .list
        .iter()
        .filter(|unit| unit.fingerprint.is_some())
        .count())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile folder that holds both layouts lists each unit with its own
    /// files, the fingerprint folder first; a build script's run has no
    /// outputs; and deleting a unit in a unit folder deletes that folder.
    #[test]
    fn a_profile_folder_lists_its_units_in_either_layout() {
        let scratch = crate::testing::Scratch::new("units-layouts");
        let profile = scratch.dir.join("debug");
        let file = |path: &str| crate::testing::fill(&profile.join(path), 1);
        file(".fingerprint/dep-aaaaaaaaaaaaaaaa/lib-dep");
        file("deps/libdep-aaaaaaaaaaaaaaaa.rlib");
        file(".fingerprint/dep-bbbbbbbbbbbbbbbb/run-build-script-build-script-build");
        file("build/dep-bbbbbbbbbbbbbbbb/out/gen.rs");
        file("build/app/cccccccccccccccc/fingerprint/lib-app");
        file("build/app/cccccccccccccccc/out/libapp-cccccccccccccccc.rlib");
        file("build/app/dddddddddddddddd/fingerprint/run-build-script-build-script-build");
        file("build/app/dddddddddddddddd/out/gen.rs");
        file("build/app/dddddddddddddddd/run/stdout");
        let units = units_in(&profile).unwrap();
        let unit = |hash: &str| {
            units
                .list
                .iter()
                .find(|unit| unit.hash == hash.repeat(16))
                .unwrap_or_else(|| panic!("no unit {hash}"))
        };
        let at =
            |paths: &[&str]| -> Vec<PathBuf> { paths.iter().map(|p| profile.join(p)).collect() };
        let sorted_after_first = |mut paths: Vec<PathBuf>| {
            paths[1..].sort();
            paths
        };
        assert_eq!(units.list.len(), 4);
        assert_eq!(
            unit("a").paths(),
            at(&[
                ".fingerprint/dep-aaaaaaaaaaaaaaaa",
                "deps/libdep-aaaaaaaaaaaaaaaa.rlib"
            ])
        );
        assert_eq!(
            unit("b").paths(),
            at(&[
                ".fingerprint/dep-bbbbbbbbbbbbbbbb",
                "build/dep-bbbbbbbbbbbbbbbb"
            ])
        );
        assert_eq!(
            unit("c").paths(),
            at(&[
                "build/app/cccccccccccccccc/fingerprint",
                "build/app/cccccccccccccccc/out"
            ])
        );
        assert_eq!(
            unit("c").outputs,
            at(&["build/app/cccccccccccccccc/out/libapp-cccccccccccccccc.rlib"])
        );
        assert_eq!(unit("c").name.as_deref(), Some("app"));
        assert_eq!(
            sorted_after_first(unit("d").paths()),
            at(&[
                "build/app/dddddddddddddddd/fingerprint",
                "build/app/dddddddddddddddd/out",
                "build/app/dddddddddddddddd/run",
            ])
        );
        assert!(
            unit("d").outputs.is_empty(),
            "a build script's run has outputs"
        );
        unit("c").delete().unwrap();
        assert!(!profile.join("build/app/cccccccccccccccc").exists());
        assert!(profile.join("build/app/dddddddddddddddd").exists());
    }
}
