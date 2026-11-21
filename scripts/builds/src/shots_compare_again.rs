//! Compares a cached shots run's pictures again, so a change to the shots
//! checker's comparison makes no run take its shots or draw its boards again.
//!
//! builds keys a cached run by the checker's capture version and, under it, by
//! its comparison version (shots_cache.rs). When a run of a commit is cached
//! under the capture version this builds is built with, and under another
//! comparison version only, the run's report holds the pictures and element
//! lists that the capture made. `builds shots recheck` of this program
//! (scripts/shots/src/recheck.rs) compares them again, and the result is
//! cached as the run of this builds' comparison version: the run's findings
//! come from the pictures with no app build. A run whose capture version is
//! another one is no source: the capture side changed, and the run takes its
//! shots again.
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
use crate::shots_cache::{self, Cached, Key};
use crate::shots_check_program;
use crate::shots_program::VERSIONS;
use crate::text::note;

/// The files of a report that hold findings, which the comparison makes again
/// and so a run made from another run does not take over.
const FINDINGS_FILES: [&str; 2] = ["index.html", shot_causes::FILE];

/// The run cached under <key>, or the run that comparing again the cached run
/// of another comparison version makes, when <key>'s capture version has one.
/// None when neither exists.
pub fn lookup(places: &Places, key: &Key) -> Result<Option<Cached>> {
    match shots_cache::lookup(places, key)? {
        Some(cached) => Ok(Some(cached)),
        None => compare_again(places, key),
    }
}

/// The run a branch's run of <prefix> on <lane> compares with
/// (`shots_cache::before_run`), made by comparing again the cached run of
/// another comparison version when this version has none.
pub fn before_run(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    prefix: &str,
) -> Result<Option<Cached>> {
    if let Some(cached) = shots_cache::before_run(places, repo, commit, lane, prefix, VERSIONS)? {
        return Ok(Some(cached));
    }
    for key in shots_cache::covering_keys(places, repo, commit, lane, prefix, VERSIONS)? {
        compare_again(places, &key)?;
    }
    shots_cache::before_run(places, repo, commit, lane, prefix, VERSIONS)
}

/// Compares again the newest cached run that has <key>'s commit, lane, prefix
/// and capture version and another comparison version, with this program, and
/// caches the result under <key>. None when no such run is cached.
fn compare_again(places: &Places, key: &Key) -> Result<Option<Cached>> {
    let program = std::env::current_exe().context(|| "finding this program".to_string())?;
    compare_again_with(places, key, &program)
}

/// `compare_again`, comparing with <program>, a builds program.
fn compare_again_with(places: &Places, key: &Key, program: &Path) -> Result<Option<Cached>> {
    let Some(source) = shots_cache::other_comparisons(places, key)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let again = shots_check_program::recheck(program, &source)?;
    let findings = findings_from(&source, &again, key)?;
    let run = shot_causes::parse(&findings)?;
    let staging = shots_cache::stage(places, key)?;
    let report = staging.report();
    link_pictures(&source.report(), &report)?;
    let file = report.join(shot_causes::FILE);
    std::fs::write(&file, &findings).context(|| format!("writing {}", file.display()))?;
    let text = format!(
        "builds shots: compared again from the run of comparison version {} with the pictures of capture version {}; it has no report page and no verdict against the base run\n{}",
        short(&source.key.comparison_version),
        short(&key.capture_version),
        shot_causes::text(&run)
    );
    let cached = staging.finish(source.exit_code, &text)?;
    note(format!(
        "compared the pictures of the cached run of {} {} {} again with comparison version {}: no app build and no shot",
        &key.commit[..key.commit.len().min(12)],
        key.lane.word(),
        key.prefix,
        short(&key.comparison_version)
    ));
    Ok(Some(cached))
}

/// The first 12 characters of a version.
fn short(version: &str) -> &str {
    &version[..version.len().min(12)]
}

/// The findings.json of the run made from <source>: its own file, with the
/// comparison version and the findings that <again>, what `builds shots
/// recheck` printed, holds. Fails when <again> is not of <key>'s versions: the
/// program that compared is not the one that keys <key>.
fn findings_from(source: &Cached, again: &str, key: &Key) -> Result<String> {
    let parse = |text: &str, what: &str| -> Result<serde_json::Value> {
        serde_json::from_str(text).context(|| format!("reading {what}"))
    };
    let mut file = parse(
        &std::fs::read_to_string(source.findings_json())
            .context(|| format!("reading {}", source.findings_json().display()))?,
        "the findings.json of the cached run",
    )?;
    let again = parse(again, "what `builds shots recheck` printed")?;
    if again["version"] != key.comparison_version || again["capture_version"] != key.capture_version
    {
        fail!(
            "`builds shots recheck` made findings of comparison version {} over capture version {}, and the run to cache is of {} over {}",
            again["version"],
            again["capture_version"],
            key.comparison_version,
            key.capture_version
        );
    }
    file["version"] = again["version"].clone();
    file["findings"] = again["findings"].clone();
    Ok(file.to_string())
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
