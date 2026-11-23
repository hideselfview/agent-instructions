//! Compares a cached shots run's pictures again, so a change to the shots
//! checker's comparison makes no run take its shots or draw its boards again.
//!
//! builds keys a cached run by the environment it was made in, the checker's
//! capture version and, under it, its comparison version (shots_cache.rs).
//! When a run of a commit is cached in an environment under the capture
//! version this builds is built with, and under another comparison version
//! only, the run's report holds the pictures and element lists that the
//! capture made. `builds shots recheck` of this program
//! (scripts/shots/src/recheck.rs) compares them again, and the result is
//! cached as the run of this builds' comparison version in the same
//! environment: the run's findings come from the pictures with no app build.
//! A run whose capture version is another one is no source: the capture side
//! changed, and the run takes its shots again. A run made in another
//! environment is no source either: its pictures are another environment's.
//!
//! A run made this way holds the pictures of its source (hard links, since the
//! cache folder is one volume), the findings the comparison made as
//! findings.json, and a text that says so. It holds no report page, and no
//! verdict against the base run: the findings are all the pairs make.

use std::path::Path;

use crate::fail;
use crate::fail::{Context, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::shot_causes;
use crate::shots_cache::{self, Cached, Key, RECHECK_NOTE};
use crate::shots_check_program;
use crate::shots_program::VERSIONS;
use crate::text::note;

/// The files of a report that hold findings, which the comparison makes again
/// and so a run made from another run does not take over.
const FINDINGS_FILES: [&str; 2] = ["index.html", shot_causes::FILE];

/// The run cached under <key>, or the run that comparing again, with the
/// builds program <program>, the cached run of another comparison version
/// makes, when <key>'s capture version has one. None when neither exists.
pub fn lookup(places: &Places, key: &Key, program: &Path) -> Result<Option<Cached>> {
    match shots_cache::lookup(places, key)? {
        Some(cached) => Ok(Some(cached)),
        None => compare_again(places, key, program),
    }
}

/// The newest cached run of <commit>'s <prefix> on <lane> made in any
/// environment (`lookup` of each environment's key), marking it used: a read
/// of a commit's own run, which says the environment the run was made in.
/// None when no environment has one.
pub fn lookup_newest(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    prefix: &str,
    program: &Path,
) -> Result<Option<Cached>> {
    let mut newest: Option<Cached> = None;
    for environment in shots_cache::environments(places, repo, commit, lane)? {
        let key = Key::new(repo, commit, lane, &environment, prefix, &VERSIONS);
        if let Some(cached) = lookup(places, &key, program)?
            && newest
                .as_ref()
                .is_none_or(|known| cached.recorded_at > known.recorded_at)
        {
            newest = Some(cached);
        }
    }
    Ok(newest)
}

/// The run a branch's run of <prefix> on <lane> compares with
/// (`shots_cache::covering_run_used`): a run made in <environment>, the branch run's
/// own, made by comparing again with the builds program <program> the cached
/// run of another comparison version when this version has none. A run made
/// in another environment is neither a before run nor a source to compare
/// again.
pub fn before_run(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    environment: &str,
    prefix: &str,
    program: &Path,
) -> Result<Option<Cached>> {
    let made = shots_cache::Made::In(environment);
    let found =
        shots_cache::covering_run_used(places, repo, commit, lane, made, prefix, &VERSIONS)?;
    if found.is_some() {
        return Ok(found);
    }
    for key in shots_cache::covering_keys(places, repo, commit, lane, made, prefix, &VERSIONS)? {
        compare_again(places, &key, program)?;
    }
    // Another writer can publish while these comparisons run. Choose from
    // the current cache, including runs that writer placed.
    shots_cache::covering_run_used(places, repo, commit, lane, made, prefix, &VERSIONS)
}

/// Compares again the newest cached run that has <key>'s commit, lane, prefix
/// and capture version and another comparison version, then caches the
/// result under <key>. None when no such run is cached.
fn compare_again(places: &Places, key: &Key, program: &Path) -> Result<Option<Cached>> {
    let Some(source) = shots_cache::other_comparisons(places, key)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let again = shots_check_program::recheck(program, &source)?;
    let (findings, passed) = findings_from(&source, &again, key)?;
    let run = shot_causes::parse(&findings)?;
    let staging = shots_cache::stage(places, key)?;
    let report = staging.report();
    link_pictures(&source.report(), &report)?;
    let file = report.join(shot_causes::FILE);
    std::fs::write(&file, &findings).context(|| format!("writing {}", file.display()))?;
    let text = format!(
        "{RECHECK_NOTE}{} with the pictures of capture version {}; it has no report page and no verdict against the base run\n{}",
        short(&source.key.versions.comparison),
        short(&key.versions.capture),
        shot_causes::text(&run)
    );
    // The status is the checker's own verdict on the run (`Outcome::passed`),
    // which `builds shots recheck` prints: with no base run, every finding
    // fails the run.
    let cached = staging.finish_recheck(&source, passed, &text)?;
    note(format!(
        "compared the pictures of the cached run of {} {} {} again with comparison version {}: no app build and no shot",
        &key.commit[..key.commit.len().min(12)],
        key.lane.word(),
        key.prefix,
        short(&key.versions.comparison)
    ));
    Ok(Some(cached))
}

/// The first 12 characters of a version.
fn short(version: &str) -> &str {
    &version[..version.len().min(12)]
}

/// The findings.json of the run made from <source>: its own file, with the
/// comparison version, the findings and the elements read that <again>, what `builds shots
/// recheck` printed, holds, and whether the checker's verdict on the run is
/// that nothing fails. Fails when <again> is not of <key>'s versions: the
/// program that compared is not the one that keys <key>.
fn findings_from(source: &Cached, again: &str, key: &Key) -> Result<(String, bool)> {
    let parse = |text: &str, what: &str| -> Result<serde_json::Value> {
        serde_json::from_str(text).context(|| format!("reading {what}"))
    };
    let mut file = parse(
        &std::fs::read_to_string(source.findings_json())
            .context(|| format!("reading {}", source.findings_json().display()))?,
        "the findings.json of the cached run",
    )?;
    let again = parse(again, "what `builds shots recheck` printed")?;
    if again["version"] != key.versions.comparison
        || again["capture_version"] != key.versions.capture
    {
        fail!(
            "`builds shots recheck` made findings of comparison version {} over capture version {}, and the run to cache is of {} over {}",
            again["version"],
            again["capture_version"],
            key.versions.comparison,
            key.versions.capture
        );
    }
    // What the comparison makes: the findings, and the elements it read for
    // them. The rest of the file records the pictures and stays.
    file["version"] = again["version"].clone();
    file["findings"] = again["findings"].clone();
    if !again["seen"].is_object() {
        fail!("`builds shots recheck` printed no elements read (`seen`) for the run");
    }
    file["seen"] = again["seen"].clone();
    let Some(passed) = again["passed"].as_bool() else {
        fail!("`builds shots recheck` printed no verdict (`passed`) on the run");
    };
    Ok((file.to_string(), passed))
}

/// Hard-links each file of the report <from> but the findings files
/// (`FINDINGS_FILES`, which sit at the report's top) to the same path under
/// <into>, which does not exist yet.
fn link_pictures(from: &Path, into: &Path) -> Result<()> {
    link_folder(from, into, true)
}

/// Hard-links the files of the folder <from> to <into>, and those of its
/// folders; <top> says <from> is the report's own folder.
fn link_folder(from: &Path, into: &Path, top: bool) -> Result<()> {
    std::fs::create_dir_all(into).context(|| format!("creating {}", into.display()))?;
    let entries = std::fs::read_dir(from).context(|| format!("listing {}", from.display()))?;
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", from.display()))?;
        let name = entry.file_name();
        let (source, target) = (entry.path(), into.join(&name));
        if source.is_dir() {
            link_folder(&source, &target, false)?;
        } else if !(top && FINDINGS_FILES.contains(&name.to_string_lossy().as_ref())) {
            std::fs::hard_link(&source, &target)
                .context(|| format!("linking {} to {}", source.display(), target.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "shots_compare_again_tests.rs"]
mod tests;
