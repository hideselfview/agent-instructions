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
    let declared: Vec<String> = declared_at(top, commit)?
        .into_iter()
        .map(|scene| scene.name)
        .collect();
    for (_, _, prefix) in wanted {
        if declared
            .iter()
            .any(|name| name.starts_with(prefix.as_str()))
        {
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
/// prefix starts a shot that the harnesses declare at <commit> as needing a
/// window. A run of the other prefixes draws no window, so it needs no window
/// lock (`Step::opens_windows`). A run of a lane's full set of shots
/// (`--all-shots`) selects every scene, so it counts when the lane declares
/// any window scene.
pub fn windowed_prefixes(top: &Path, commit: &str, picks: &[Pick]) -> Result<Vec<String>> {
    let wanted: Vec<&String> = picks
        .iter()
        .filter(|pick| pick.1 == PickKind::Shots)
        .map(|pick| &pick.2)
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let declared = declared_at(top, commit)?;
    Ok(wanted
        .into_iter()
        .filter(|prefix| {
            declared
                .iter()
                .any(|scene| scene.needs_window && scene.name.starts_with(prefix.as_str()))
        })
        .cloned()
        .collect())
}

/// The shots the harnesses declare at <commit>, read from the declare
/// folders that the commit's `.app-build` names.
fn declared_at(top: &Path, commit: &str) -> Result<Vec<shots::Scene>> {
    let config = run_git(top, &["show", &format!("{commit}:{CONFIG}")])?;
    let folders = boards::app_build::declared(&config).map_err(|e| Fail::Message(e.text()))?;
    let paths: Vec<String> = folders.into_iter().map(|(_, folder)| folder).collect();
    if paths.is_empty() {
        fail!("{CONFIG} at {commit} has no shots-declared line, so no shot is declared");
    }
    let scratch = std::env::temp_dir();
    crate::evict::in_temp_dir(&scratch, ".builds-declared.", |root| {
        boards::export(top, commit, &paths, root, "the declared shots")?;
        shots::declared_scenes(root, &config).map_err(Fail::Message)
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
