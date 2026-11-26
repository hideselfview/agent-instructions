//! The lanes of a repo: the apps its `.app-build` names. A command that has a
//! commit reads them from that commit's config (`Config::lanes`); a command
//! with only the repo reads them from the `.app-build` that origin/main has in
//! the repo's local source (`of`), or from the files the repo's folder on the
//! volume holds for each app (`with_files`).

use crate::app_config::{CONFIG, config_in};
use crate::app_remote::find_local_source;
use crate::fail::Result;
use crate::lane::{Lane, app_of_file};
use crate::places::Places;

/// The lanes <repo> has at origin/main, in the platforms' order; none when
/// the repo has no `.app-build` there, so no app builds.
pub fn of(places: &Places, repo: &str) -> Result<Vec<Lane>> {
    match find_local_source(places, repo)? {
        Some(source) => in_source(&source),
        None => Ok(Vec::new()),
    }
}

/// The lanes at origin/main of the repo whose checkout or clone is at
/// <source>.
pub fn in_source(source: &std::path::Path) -> Result<Vec<Lane>> {
    let named = format!("origin/main:{CONFIG}");
    let has_config = std::process::Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["cat-file", "-e", &named])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !has_config {
        return Ok(Vec::new());
    }
    Ok(config_in(source, "origin/main")?.lanes)
}

/// The lanes of <repo> that have a set of app checkouts, a removal record or
/// a lock folder in the repo's folder, in the platforms' order. An app named
/// for a platform needs no `.app-build` read; a file of any other app names a
/// lane only while origin/main's `.app-build` still names the app.
pub fn with_files(places: &Places, repo: &str) -> Result<Vec<Lane>> {
    let dir = places.root.join(repo);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => crate::fail!("listing {}: {error}", dir.display()),
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| app_of_file(&entry.file_name().to_string_lossy()).map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    let mut lanes = Vec::new();
    let mut configured: Option<Vec<Lane>> = None;
    for name in names {
        if let Some(lane) = Lane::parse(&name) {
            lanes.push(lane);
            continue;
        }
        let configured = match &configured {
            Some(configured) => configured,
            None => configured.insert(of(places, repo)?),
        };
        lanes.extend(configured.iter().filter(|lane| lane.word() == name));
    }
    lanes.sort();
    lanes.dedup();
    Ok(lanes)
}

/// The lane of <repo> that <word> names: the app named for a platform, or an
/// app of the repo's `.app-build` at origin/main; fails naming the apps there
/// are.
pub fn parse(places: &Places, repo: &str, word: &str) -> Result<Lane> {
    if let Some(lane) = Lane::parse(word) {
        return Ok(lane);
    }
    Lane::parse_in_or_fail(&of(places, repo)?, word)
}

/// `parse`, reading the repo's apps from the checkout at <source>.
pub fn parse_in_source(source: &std::path::Path, word: &str) -> Result<Lane> {
    if let Some(lane) = Lane::parse(word) {
        return Ok(lane);
    }
    Lane::parse_in_or_fail(&in_source(source)?, word)
}
