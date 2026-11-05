//! Cargo's git checkouts (<cargo home>/git/checkouts/<repo>-<hash>/<commit>),
//! one per commit any build fetched, which cargo never deletes: every coven pin
//! bump in bae adds one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::remove;
use crate::fail::Result;
use crate::text::{counted, human_bytes, note};
use crate::usage::{Seen, bytes_with};

/// The git commits <lockfiles> (Cargo.lock files) pin: each
/// `source = "git+...#<sha>"` line.
fn pinned_revisions(lockfiles: &[PathBuf]) -> Result<HashSet<String>> {
    let mut pinned = HashSet::new();
    for lockfile in lockfiles {
        let text = match std::fs::read_to_string(lockfile) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => crate::fail!("reading {}: {error}", lockfile.display()),
        };
        for line in text.lines() {
            let Some(source) = line.strip_prefix("source = \"git+").and_then(|rest| rest.strip_suffix('"')) else {
                continue;
            };
            let Some((before, sha)) = source.split_once('#') else { continue };
            if before.contains('"')
                || sha.len() != 40
                || !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                continue;
            }
            pinned.insert(sha.to_string());
        }
    }
    Ok(pinned)
}

/// Deletes each checkout no Cargo.lock in <lockfiles> pins, while holding
/// cargo's package cache locks, which a cargo checking one out holds. The git
/// databases (git/db) stay: cargo checks a commit out of them again in
/// seconds. Returns the KiB freed.
pub fn prune_git_checkouts(cargo_home: &Path, dry: bool, lockfiles: &[PathBuf]) -> Result<u64> {
    let checkouts = cargo_home.join("git").join("checkouts");
    if !checkouts.is_dir() {
        return Ok(0);
    }
    let pinned = pinned_revisions(lockfiles)?;
    let mut locks = Vec::new();
    if !dry {
        for name in [".package-cache", ".package-cache-mutate"] {
            match crate::evict::try_lock_creating(&cargo_home.join(name))? {
                Some(lock) => locks.push(lock),
                None => {
                    note(format!("kept cargo's git checkouts in {}: a cargo holds its package cache", checkouts.display()));
                    return Ok(0);
                }
            }
        }
    }
    let mut repos = super::names_in(&checkouts)?;
    repos.sort();
    let mut unpinned = Vec::new();
    for repo in repos {
        let repo_dir = checkouts.join(&repo);
        if !repo_dir.is_dir() {
            continue;
        }
        let mut shorts = super::names_in(&repo_dir)?;
        shorts.sort();
        for short in shorts {
            let path = repo_dir.join(&short);
            if path.is_dir() && !pinned.iter().any(|sha| sha.starts_with(&short)) {
                unpinned.push(path);
            }
        }
    }
    let seen = Seen::default();
    let mut size = 0;
    for path in &unpinned {
        size += bytes_with(&seen, path)?;
    }
    if !unpinned.is_empty() {
        if !dry {
            for path in &unpinned {
                remove(path)?;
            }
        }
        note(format!(
            "{} {} no Cargo.lock pins from {} ({})",
            if dry { "would delete" } else { "deleted" },
            counted(unpinned.len(), "git checkout", "git checkouts"),
            checkouts.display(),
            human_bytes(size)
        ));
    }
    drop(locks);
    Ok(size / 1024)
}
