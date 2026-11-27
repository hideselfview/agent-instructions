//! Saves as one run of a shots prefix the cached runs of narrower prefixes
//! that together tried every shot the prefix selects.
//!
//! A cached run holds the findings of each scene it checked, and the shots of
//! a prefix are the scenes the commit declares under it. So when the runs of
//! one commit, lane, environment and shots program tried every declared scene
//! of a prefix, the cache holds all that a run of the prefix would find, and
//! builds saves it as that run: every reader of a covering run
//! (shots_cache.rs `covering_run`) then finds it.
//!
//! The run takes each scene from the newest run that tried it: its findings,
//! board hash, elements, sizes and capture failure. Its report links the
//! pictures of those runs, and it has no report page. Its verdict fails when
//! the verdict of any of those runs failed.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::fail;
use crate::fail::{Context, Result};
use crate::output::note;
use crate::places::Places;
use crate::shot_causes;
use crate::shots_cache::{self, Cached, Key};

/// The parts of a findings.json that hold one entry for each scene, by its
/// name.
const BY_SCENE: [&str; 3] = ["boards", "seen", "sizes"];

/// A cached run with its findings.json.
struct Part {
    run: Cached,
    file: Value,
}

impl Part {
    fn read(run: Cached) -> Result<Part> {
        let path = run.findings_json();
        let text =
            std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
        let file = serde_json::from_str(&text).context(|| format!("reading {}", path.display()))?;
        Ok(Part { run, file })
    }

    /// The list <name> of the file.
    fn list(&self, name: &str) -> Result<&[Value]> {
        match self.file[name].as_array() {
            Some(list) => Ok(list),
            None => fail!(
                "{} holds no list `{name}`",
                self.run.findings_json().display()
            ),
        }
    }

    /// The scenes the run tried: the ones it checked and the ones its capture
    /// wrote no usable shot of.
    fn tried(&self) -> Result<BTreeSet<&str>> {
        let checked = self.list("scenes")?.iter().map(Value::as_str);
        let failed = self.list("failed")?.iter().map(|one| one["scene"].as_str());
        match checked.chain(failed).collect() {
            Some(tried) => Ok(tried),
            None => fail!(
                "{} names a scene by no text",
                self.run.findings_json().display()
            ),
        }
    }
}

/// Makes the run <key> from the cached runs of narrower prefixes, when they
/// tried every shot under <key>'s prefix that <declared> lists, caches it and
/// returns it; None when no such runs tried them all, or when <declared>
/// lists no shot under the prefix.
pub fn compose(
    places: &Places,
    key: &Key,
    declared: &mut dyn FnMut() -> Result<Vec<String>>,
) -> Result<Option<Cached>> {
    // The cap deletes no run while this reads them.
    let _reading = shots_cache::writing(places)?;
    let mut parts = Vec::new();
    for part in shots_cache::narrower_keys(places, key)? {
        if let Some(run) = shots_cache::lookup(places, &part)?
            && run.findings_json().is_file()
        {
            parts.push(Part::read(run)?);
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    parts.sort_by_key(|part| std::cmp::Reverse(part.run.recorded_at()));
    let mut wanted: BTreeSet<String> = declared()?
        .into_iter()
        .filter(|name| name.starts_with(&key.prefix))
        .collect();
    if wanted.is_empty() {
        return Ok(None);
    }
    // Each part that gives a scene, with the scenes it gives.
    let mut chosen: Vec<(&Part, BTreeSet<String>)> = Vec::new();
    for part in &parts {
        let tried = part.tried()?;
        let gives: BTreeSet<String> = wanted
            .iter()
            .filter(|name| tried.contains(name.as_str()))
            .cloned()
            .collect();
        wanted.retain(|name| !gives.contains(name));
        if !gives.is_empty() {
            chosen.push((part, gives));
        }
    }
    if !wanted.is_empty() {
        return Ok(None);
    }
    let findings = findings_of(&chosen)?.to_string();
    let run = shot_causes::parse(&findings)?;
    let staging = shots_cache::stage(places, key)?;
    let report = staging.report();
    for (part, _) in &chosen {
        crate::shots_compare_again::link_pictures(&part.run.report(), &report)?;
    }
    let file = report.join(shot_causes::FILE);
    std::fs::write(&file, &findings).context(|| format!("writing {}", file.display()))?;
    let text = format!(
        "builds shots: put together from the saved runs of {}; it has no report page\n{}",
        chosen
            .iter()
            .map(|(part, _)| part.run.key.prefix.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        shot_causes::text(&run)
    );
    let failed = chosen.iter().any(|(part, _)| part.run.app_exit_code() != 0);
    let ended = chosen
        .iter()
        .map(|(part, _)| part.run.recorded_at())
        .max()
        .expect("a wanted scene has a part that gives it");
    let cached = staging.finish_at(i64::from(failed), &text, ended)?;
    note(format!(
        "the saved runs of {} {} under {} tried every shot it selects, so builds saved them as its run: no app build and no shot",
        &key.commit[..key.commit.len().min(12)],
        key.lane.word(),
        key.prefix
    ));
    Ok(Some(cached))
}

/// The findings.json of the run that takes from each of <chosen>, newest
/// first, the scenes it gives. What the file says of no one scene comes from
/// the newest.
fn findings_of(chosen: &[(&Part, BTreeSet<String>)]) -> Result<Value> {
    let mut file = chosen[0].0.file.clone();
    let mut scenes = BTreeSet::new();
    let mut failed = Vec::new();
    let mut findings = Vec::new();
    let mut by_scene: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    let mut problems = 0;
    for (part, gives) in chosen {
        let given = |scene: &Value| scene.as_str().is_some_and(|name| gives.contains(name));
        scenes.extend(
            part.list("scenes")?
                .iter()
                .filter(|scene| given(scene))
                .filter_map(Value::as_str),
        );
        failed.extend(
            part.list("failed")?
                .iter()
                .filter(|one| given(&one["scene"]))
                .cloned(),
        );
        findings.extend(
            part.list("findings")?
                .iter()
                .filter(|one| given(&one["scene"]))
                .cloned(),
        );
        for name in BY_SCENE {
            let entries = by_scene.entry(name).or_default();
            for scene in gives {
                if let Some(entry) = part.file[name].get(scene) {
                    entries.insert(scene.clone(), entry.clone());
                }
            }
        }
        // The count names no scene, so two parts may have counted one
        // problem: the largest is not zero when any part found one, which
        // fails a recheck.
        problems = problems.max(part.file["problems"].as_u64().unwrap_or_default());
    }
    file["scenes"] = scenes.into_iter().collect::<Vec<_>>().into();
    file["failed"] = failed.into();
    file["findings"] = findings.into();
    file["problems"] = problems.into();
    for (name, entries) in by_scene {
        file[name] = Value::Object(entries);
    }
    Ok(file)
}
