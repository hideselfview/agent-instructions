//! This module lists the workspace crates whose resolved dependencies changed
//! between two versions of `Cargo.lock`. A lock entry changed when the new
//! lock has it (by name, version and source) and the old one has it otherwise
//! or not at all. A workspace crate's dependencies changed when its own entry
//! changed or one of the entries it depends on, at any depth, did. When the
//! new lock no longer has a package, the entry of each package that depended
//! on it changed too, so the module finds the same crates through those
//! entries.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::fail;
use crate::fail::{Context, Result};

/// One package of a lock: its name, version and source (empty for a
/// workspace crate or a path dependency).
type Key = (String, String, String);

/// A lock's packages by key, with each one's whole entry and the keys of the
/// packages it depends on.
struct Lock {
    entries: HashMap<Key, toml::Value>,
    depends: HashMap<Key, Vec<Key>>,
}

/// Reads the text of a `Cargo.lock`; <what> names it in errors.
fn parse(text: &str, what: &str) -> Result<Lock> {
    let value: toml::Value = toml::from_str(text).context(|| format!("reading {what}"))?;
    let packages = match value.get("package") {
        Some(toml::Value::Array(packages)) => packages.as_slice(),
        Some(_) => fail!("{what}'s package is not a list"),
        None => &[],
    };
    // A field's text; None when the entry has no such field.
    let text_of = |package: &toml::Value, field: &str| -> Result<Option<String>> {
        match package.get(field) {
            None => Ok(None),
            Some(toml::Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
            Some(other) => fail!("{what} has a package whose {field} is {other}, not text"),
        }
    };
    let mut entries = HashMap::new();
    for package in packages {
        let Some(name) = text_of(package, "name")? else {
            fail!("{what} has a package without a name");
        };
        let Some(version) = text_of(package, "version")? else {
            fail!("{what} has a package {name} without a version");
        };
        // A workspace crate or a path dependency has no source.
        let source = text_of(package, "source")?.unwrap_or_default();
        entries.insert((name, version, source), package.clone());
    }
    let mut depends = HashMap::new();
    for (key, package) in &entries {
        let mut keys = Vec::new();
        let listed = match package.get("dependencies") {
            None => &[][..],
            Some(toml::Value::Array(list)) => list.as_slice(),
            Some(_) => fail!("{what}: the dependencies of {} are not a list", key.0),
        };
        for dependency in listed {
            let Some(text) = dependency.as_str() else {
                fail!("{what}: a dependency of {} is not text", key.0);
            };
            keys.push(resolve(&entries, text).map_err(|fail| {
                crate::fail::Fail::Message(format!("reading {what}: {}", fail.text()))
            })?);
        }
        depends.insert(key.clone(), keys);
    }
    Ok(Lock { entries, depends })
}

/// The key a lock's dependency text names: `name`, `name version` or
/// `name version (source)`, where the name alone is unique in the lock.
fn resolve(entries: &HashMap<Key, toml::Value>, text: &str) -> Result<Key> {
    let mut words = text.splitn(3, ' ');
    let name = words.next().unwrap_or_default();
    let version = words.next();
    let source = words
        .next()
        .map(|source| source.trim_start_matches('(').trim_end_matches(')'));
    let found: Vec<&Key> = entries
        .keys()
        .filter(|key| {
            key.0 == name
                && version.is_none_or(|version| key.1 == version)
                && source.is_none_or(|source| key.2 == source)
        })
        .collect();
    match found[..] {
        [key] => Ok(key.clone()),
        [] => fail!("the dependency {text:?} names no package of the lock"),
        _ => fail!("the dependency {text:?} names more than one package of the lock"),
    }
}

/// The sorted names of the <members> whose own lock entry, or an entry they
/// depend on at any depth, differs between the lock texts <old> and <new>.
pub fn with_changed_dependencies(old: &str, new: &str, members: &[String]) -> Result<Vec<String>> {
    let old = parse(old, "the old Cargo.lock")?;
    let new = parse(new, "the new Cargo.lock")?;
    let changed: BTreeSet<&Key> = new
        .entries
        .iter()
        .filter(|(key, entry)| old.entries.get(*key) != Some(*entry))
        .map(|(key, _)| key)
        .collect();
    let mut changed_members = Vec::new();
    for member in members {
        let starts: Vec<&Key> = new
            .entries
            .keys()
            .filter(|key| key.0 == *member && key.2.is_empty())
            .collect();
        let mut seen: BTreeSet<&Key> = BTreeSet::new();
        let mut stack = starts;
        let mut hit = false;
        while let Some(key) = stack.pop() {
            if !seen.insert(key) {
                continue;
            }
            if changed.contains(key) {
                hit = true;
                break;
            }
            stack.extend(new.depends.get(key).into_iter().flatten());
        }
        if hit {
            changed_members.push(member.clone());
        }
    }
    changed_members.sort();
    Ok(changed_members)
}

/// The `Cargo.lock` of commit <rev> in the repo at <top>; None when <rev> is
/// no commit (a repo before its first) or that commit has no `Cargo.lock`.
pub fn lock_at(top: &Path, rev: &str) -> Result<Option<String>> {
    let answers = |args: &[&str]| -> Result<bool> {
        Ok(boards::git(top)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context(|| "running git".to_string())?
            .success())
    };
    if !answers(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{rev}^{{commit}}"),
    ])? || !answers(&["cat-file", "-e", &format!("{rev}:Cargo.lock")])?
    {
        return Ok(None);
    }
    let text = boards::git_text(
        boards::git(top).args(["show", &format!("{rev}:Cargo.lock")]),
        "read the old Cargo.lock",
    )?;
    Ok(Some(text))
}

#[cfg(test)]
#[path = "lock_diff_tests.rs"]
mod tests;
