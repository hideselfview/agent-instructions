//! Whether a `builds cargo` job needs a Mac: it builds a package that the
//! checkout's `.build-dirs` names in a `macos` line, one part of whose code
//! compiles only for macOS. On Linux such a job would compile and check
//! other code than on a Mac, so `builds cargo` runs it only on a Mac
//! (cargo_place.rs). A job builds the packages its `-p` arguments name; with
//! none, every package of the workspace with `--workspace`, else the package
//! of the folder it runs in, or every package when that folder is the
//! workspace's own.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail::{Context, Result};

/// The package that keeps the job <args>, run in <dir> of <checkout>, on a
/// Mac, or None when the job builds none of the packages the checkout's
/// `.build-dirs` names in `macos` lines.
pub fn macos_package(args: &[String], checkout: &Checkout, dir: &Path) -> Result<Option<String>> {
    let macos = crate::manifest::macos_packages(&checkout.top)?;
    if macos.is_empty() {
        return Ok(None);
    }
    let selected = selection(args);
    let found = match selected.named.is_empty() {
        false => macos
            .iter()
            .find(|package| selected.named.iter().any(|spec| matches(spec, package))),
        true if selected.workspace => macos
            .iter()
            .find(|package| !selected.excluded.iter().any(|spec| matches(spec, package))),
        true => match package_of(&checkout.top, dir)? {
            Some(name) => macos.iter().find(|package| **package == name),
            None => macos.first(),
        },
    };
    Ok(found.cloned())
}

/// The packages cargo's own arguments name (those before any `--`).
#[derive(Debug, Default, PartialEq)]
struct Selection {
    named: Vec<String>,
    workspace: bool,
    excluded: Vec<String>,
}

fn selection(args: &[String]) -> Selection {
    let mut selected = Selection::default();
    let mut words = args.iter().take_while(|arg| *arg != "--");
    while let Some(word) = words.next() {
        let (flag, value) = match word.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (word.as_str(), None),
        };
        let mut value = || value.clone().or_else(|| words.next().cloned());
        match flag {
            "-p" | "--package" => selected.named.extend(value()),
            "--exclude" => selected.excluded.extend(value()),
            "--workspace" | "--all" => selected.workspace = true,
            short if short.starts_with("-p") && short.len() > 2 => {
                selected.named.push(short[2..].to_string())
            }
            _ => {}
        }
    }
    selected
}

/// Does the package spec <spec> (`bae-util`, `bae-util@0.1.0`, `bae-*`)
/// name <package>?
fn matches(spec: &str, package: &str) -> bool {
    let name = spec.split('@').next().unwrap_or(spec);
    match name.split_once('*') {
        Some((start, end)) => {
            package.len() >= start.len() + end.len()
                && package.starts_with(start)
                && package.ends_with(end)
        }
        None => name == package,
    }
}

/// The package whose folder holds <dir>, the nearest up to <top>; None when
/// the nearest manifest is the workspace's and names no package of its own.
fn package_of(top: &Path, dir: &Path) -> Result<Option<String>> {
    let mut at = Some(dir);
    while let Some(folder) = at.filter(|folder| folder.starts_with(top)) {
        let manifest = folder.join("Cargo.toml");
        if manifest.is_file() {
            let text = std::fs::read_to_string(&manifest)
                .context(|| format!("reading {}", manifest.display()))?;
            let table: toml::Table =
                toml::from_str(&text).context(|| format!("reading {}", manifest.display()))?;
            return Ok(table
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(|name| name.as_str())
                .map(str::to_string));
        }
        at = folder.parent();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, repo};

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_job_needs_a_mac_when_it_builds_a_package_a_macos_line_names() {
        let scratch = Scratch::new("cargo-needs");
        let top = scratch.dir.join("demo");
        repo(&top, "target 1G\nmacos playback util\n");
        std::fs::write(top.join("Cargo.toml"), "[workspace]\nmembers = [\"*\"]\n").unwrap();
        for name in ["playback", "store"] {
            std::fs::create_dir_all(top.join(name).join("src")).unwrap();
            std::fs::write(
                top.join(name).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\n"),
            )
            .unwrap();
        }
        let checkout = Checkout::at(&top).unwrap().unwrap();
        let needs = |args: &str, dir: &Path| macos_package(&words(args), &checkout, dir).unwrap();
        assert_eq!(needs("test -p playback", &top), Some("playback".into()));
        assert_eq!(
            needs("test --package=util@0.1.0", &top),
            Some("util".into())
        );
        assert_eq!(needs("clippy -pplay*", &top), Some("playback".into()));
        assert_eq!(needs("test -p store", &top), None);
        assert_eq!(needs("test -p store -- -p playback", &top), None);
        assert_eq!(needs("check --workspace", &top), Some("playback".into()));
        assert_eq!(
            needs("check --workspace --exclude playback --exclude util", &top),
            None
        );
        assert_eq!(needs("test", &top.join("store/src")), None);
        assert_eq!(
            needs("test", &top.join("playback")),
            Some("playback".into())
        );
        assert_eq!(
            needs("test", &top),
            Some("playback".into()),
            "the workspace's own folder"
        );
        repo(&scratch.dir.join("plain"), "target 1G\n");
        let plain = Checkout::at(&scratch.dir.join("plain")).unwrap().unwrap();
        assert_eq!(
            macos_package(&words("test --workspace"), &plain, &plain.top).unwrap(),
            None
        );
    }
}
