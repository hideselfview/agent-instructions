//! Refuses a `--shots` prefix that starts no declared shot when an agent
//! starts an app build, so a mistyped prefix costs no turn in a lane's queue.
//! The declared shots come from the harnesses' source at the build's commit
//! (the `shots-declared` lines of its `.app-build`, read by the shots checker),
//! so the check runs no test and builds no app. A prefix whose run the store
//! already caches for the commit needs no check: app-build prints that run
//! first (app_build.rs `serve_cached`).

use std::path::Path;

use crate::app_build::{self, Pick};
use crate::app_config::{CONFIG, PickKind, run_git};
use crate::fail;
use crate::fail::{Fail, Result};
use crate::lane::Lane;

/// How many nearest prefixes a refusal names.
const NEAREST: usize = 6;

/// Fails when a shots pick of <picks> starts no shot that the harnesses
/// declare at <commit> of the repo at <top>, and names the nearest declared
/// prefixes. A lane's full set (`--all-shots`) needs no declared name.
pub fn refuse_undeclared(top: &Path, commit: &str, picks: &[Pick]) -> Result<()> {
    let wanted: Vec<&Pick> = picks
        .iter()
        .filter(|pick| pick.1 == PickKind::Shots && **pick != app_build::all_shots(pick.0))
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let declared = declared_names_at(top, commit)?;
    for (_, _, prefix) in wanted {
        if starts_a_shot(&declared, prefix) {
            continue;
        }
        let near = nearest(&declared, prefix);
        let hint = if near.is_empty() {
            "The repo declares no shot on that platform.".to_string()
        } else {
            format!("The nearest declared prefixes are {}.", near.join(", "))
        };
        fail!(
            "no shot that the repo declares at {commit} starts with {prefix}, so the build would fail after it waited in its lane's queue. {hint}"
        );
    }
    Ok(())
}

/// The shots of <picks> that select a scene shown in its window, named by
/// their prefixes, in the order of <picks>: the shots picks of any lane whose
/// prefix starts a shot that the harnesses declare at <commit> as shown in a
/// window. Every such scene draws on a shot display of its host's, which a
/// second run on the host would add or remove while the first draws, and a
/// scene that takes the keyboard focus (`takesFocus: true`) also moves the
/// focus, so such a run holds the host's window lock (`Step::opens_windows`).
/// A run of the other prefixes draws no window and needs no lock. A run of a
/// lane's full set of shots (`--all-shots`) selects every scene, so it counts
/// when the lane declares any window scene.
pub fn windowed_prefixes(top: &Path, commit: &str, picks: &[Pick]) -> Result<Vec<String>> {
    let wanted: Vec<&String> = picks
        .iter()
        .filter(|pick| pick.1 == PickKind::Shots)
        .map(|pick| &pick.2)
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let declared = declared_at(top, commit, None)?;
    Ok(wanted
        .into_iter()
        .filter(|prefix| {
            declared.iter().any(|scene| {
                scene.marks.in_window && scene.name.to_string().starts_with(prefix.as_str())
            })
        })
        .cloned()
        .collect())
}

/// Whether a name of <declared> starts with <prefix>.
pub fn starts_a_shot(declared: &[String], prefix: &str) -> bool {
    declared.iter().any(|name| name.starts_with(prefix))
}

/// The names of the shots the harnesses declare at <commit> of the repo at
/// <top>.
pub fn declared_names_at(top: &Path, commit: &str) -> Result<Vec<String>> {
    declared_names(top, commit, None)
}

/// The names declared by the harness for <lane>, empty when the commit
/// names no harness for that lane.
pub fn declared_names_in_lane(top: &Path, commit: &str, lane: Lane) -> Result<Vec<String>> {
    declared_names(top, commit, Some(lane))
}

fn declared_names(top: &Path, commit: &str, lane: Option<Lane>) -> Result<Vec<String>> {
    Ok(declared_at(top, commit, lane)?
        .into_iter()
        .map(|scene| scene.name.to_string())
        .collect())
}

/// The shots the commit's `.app-build` declares, limited to <lane> when given.
fn declared_at(top: &Path, commit: &str, lane: Option<Lane>) -> Result<Vec<shots::Declared>> {
    let config = run_git(top, &["show", &format!("{commit}:{CONFIG}")])?;
    let folders = match lane {
        Some(lane) => {
            let platform = lane.shots_platform();
            let folder = boards::app_build::declared_folder(&config, platform)
                .map_err(|error| Fail::Message(error.text()))?;
            let Some(folder) = folder else {
                return Ok(Vec::new());
            };
            vec![(platform, folder)]
        }
        None => shots::declared_folders(&config).map_err(Fail::Message)?,
    };
    if folders.is_empty() {
        fail!("{CONFIG} at {commit} has no shots-declared line, so no shot is declared");
    }
    let paths: Vec<String> = folders.iter().map(|(_, folder)| folder.clone()).collect();
    let scratch = std::env::temp_dir();
    crate::evict::in_temp_dir(&scratch, ".builds-declared.", |root| {
        boards::export(top, commit, &paths, root, "the declared shots")?;
        shots::declared_scenes(root, &folders).map_err(Fail::Message)
    })
}

/// The declared prefixes closest to <prefix>: drop its last words until some
/// declared name starts with what is left, then list the next word of each
/// name that does, with the dot when the name goes on. Prefixes that share
/// the most with <prefix> come first.
fn nearest(declared: &[String], prefix: &str) -> Vec<String> {
    let mut stem = prefix;
    let stem = loop {
        let Some(dot) = stem.trim_end_matches('.').rfind('.') else {
            return Vec::new();
        };
        stem = &stem[..=dot];
        if declared.iter().any(|name| name.starts_with(stem)) {
            break stem;
        }
    };
    let mut found: Vec<String> = declared
        .iter()
        .filter_map(|name| name.strip_prefix(stem))
        .map(|rest| match rest.find('.') {
            Some(dot) => rest[..=dot].to_string(),
            None => rest.to_string(),
        })
        .map(|rest| format!("{stem}{rest}"))
        .collect();
    found.sort();
    found.dedup();
    let shared = |candidate: &String| {
        candidate
            .bytes()
            .zip(prefix.bytes())
            .take_while(|(a, b)| a == b)
            .count()
    };
    found.sort_by_key(|candidate| std::cmp::Reverse(shared(candidate)));
    found.truncate(NEAREST);
    found
}

#[cfg(test)]
#[path = "shots_declared_tests.rs"]
mod tests;
