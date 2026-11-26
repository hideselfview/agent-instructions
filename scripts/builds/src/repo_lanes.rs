//! The lanes of a repo: the apps its `.app-build` names. A command that has a
//! commit reads them from that commit's config (`Config::lanes`); a command
//! with only the repo reads them from the `.app-build` that origin/main has in
//! the repo's local source (`of`), and from the files the repo's folder on the
//! volume holds for each app (`with_files`).

use std::path::Path;

use crate::app_config::{CONFIG, config_in};
use crate::app_remote::local_source;
use crate::fail::Result;
use crate::lane::{Lane, app_of_file};
use crate::places::Places;

/// The lanes <repo> has at origin/main, in the platforms' order. Fails when
/// the repo has no checkout or clone here to read them from.
pub fn of(places: &Places, repo: &str) -> Result<Vec<Lane>> {
    in_source(&local_source(places, repo)?)
}

/// The lanes at origin/main of the repo whose checkout or clone is at
/// <source>; none when origin/main has no `.app-build`, so no app builds.
pub fn in_source(source: &Path) -> Result<Vec<Lane>> {
    if !boards::exists_in(source, "origin/main", CONFIG)
        .map_err(|error| crate::fail::Fail::Message(error.to_string()))?
    {
        return Ok(Vec::new());
    }
    Ok(config_in(source, "origin/main")?.lanes)
}

/// The lanes of <repo> that have a set of app checkouts, a removal record or
/// a lock folder in the repo's folder, in the platforms' order. Only a file of
/// an app that is not named for its platform needs origin/main's
/// `.app-build`; builds names each file of an app that it no longer names.
pub fn with_files(places: &Places, repo: &str) -> Result<Vec<Lane>> {
    let mut lanes = Vec::new();
    let mut unnamed = Vec::new();
    for name in places.repo_entry_names(repo)? {
        let Some(app) = app_of_file(&name) else {
            continue;
        };
        match Lane::named_for(app) {
            Some(lane) => lanes.push(lane),
            None => unnamed.push((app.to_string(), name)),
        }
    }
    if !unnamed.is_empty() {
        let configured = of(places, repo)?;
        for (app, file) in unnamed {
            match configured.iter().find(|lane| lane.word() == app) {
                Some(lane) => lanes.push(*lane),
                None => crate::text::note(format!(
                    "{repo}'s folder on the volume holds {file}, and {CONFIG} at origin/main names no app {app}, so builds leaves that app out"
                )),
            }
        }
    }
    lanes.sort();
    lanes.dedup();
    Ok(lanes)
}

/// The lane of <repo> that <word> names: the app named for a platform, which
/// needs no line, else the app that origin/main's `.app-build` names so;
/// fails naming the apps there are.
pub fn parse(places: &Places, repo: &str, word: &str) -> Result<Lane> {
    match Lane::named_for(word) {
        Some(lane) => Ok(lane),
        None => Lane::parse_in_or_fail(&of(places, repo)?, word),
    }
}

/// `parse`, reading the repo's apps from the checkout at <source>.
pub fn parse_in_source(source: &Path, word: &str) -> Result<Lane> {
    match Lane::named_for(word) {
        Some(lane) => Ok(lane),
        None => Lane::parse_in_or_fail(&in_source(source)?, word),
    }
}
