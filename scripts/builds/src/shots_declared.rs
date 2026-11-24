//! Refuses a `--shots` prefix that starts no declared shot when an agent
//! starts an app build, so a mistyped prefix costs no turn in a lane's queue.
//! The declared shots come from the harnesses' source at the build's commit
//! (the `shots-declared` lines of its `.app-build`, read by the shots checker),
//! so the check runs no test and builds no app. A prefix whose run the store
//! already caches for the commit needs no check: app-build prints that run
//! first (app_build.rs `serve_cached`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use crate::app_build::Pick;
use crate::app_config::{CONFIG, run_git};
use crate::fail;
use crate::fail::{Fail, Result};
use crate::lane::Lane;
use crate::text::note;

/// How many nearest prefixes a refusal names.
const NEAREST: usize = 6;

/// Fails when a shots pick of <picks> starts no shot that the harnesses
/// declare for its own lane at <commit> of the repo at <top>, and names the
/// nearest declared prefixes. A lane's full set (`--all-shots`) needs no
/// declared name, and a lane no pick names is not read.
pub fn refuse_undeclared(top: &Path, commit: &str, picks: &[Pick]) -> Result<()> {
    let mut declared: HashMap<Lane, Vec<String>> = HashMap::new();
    for pick in picks {
        let Pick::Shots(lane, prefix) = pick else {
            continue;
        };
        if pick.is_all_shots() {
            continue;
        }
        let prefix = prefix.as_str();
        if !declared.contains_key(lane) {
            declared.insert(*lane, declared_names_in_lane(top, commit, *lane)?);
        }
        let declared = &declared[lane];
        if starts_a_shot(declared, prefix) {
            continue;
        }
        let near = nearest(declared, prefix);
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

/// The shots prefixes that need a display, with the access each needs.
/// A Mac harness that passes `--windows` and has no `shared-shot-display`
/// line adds its own display even for a bitmap capture. A sharing harness opens a window only when a
/// selected scene declares one.
pub fn windowed_prefixes(
    top: &Path,
    commit: &str,
    config: &crate::app_config::Config,
    picks: &[Pick],
) -> Result<Vec<(String, crate::window_lock::Access)>> {
    use crate::window_lock::Access;
    let adds_display = config
        .picker_in(Lane::Mac, crate::app_config::PickKind::Shots, None)
        .is_some_and(|picker| picker.opens_windows)
        && !config.mac_shares_shot_display;
    let mut declared: HashMap<Lane, Vec<shots::Declared>> = HashMap::new();
    let mut windowed = Vec::new();
    for pick in picks {
        let Pick::Shots(lane, prefix) = pick else {
            continue;
        };
        let prefix = prefix.as_str();
        if adds_display && prefix.starts_with("mac.") {
            windowed.push((prefix.to_string(), Access::Exclusive));
            continue;
        }
        if !declared.contains_key(lane) {
            declared.insert(*lane, declared_at(top, commit, Some(*lane))?);
        }
        let access = declared[lane]
            .iter()
            .filter(|scene| scene.name.to_string().starts_with(prefix))
            .filter_map(|scene| {
                if scene.marks.takes_focus
                    || (scene.marks.in_window && !config.mac_shares_shot_display)
                {
                    Some(Access::Exclusive)
                } else if scene.marks.in_window {
                    Some(Access::Shared)
                } else {
                    None
                }
            })
            .max();
        if let Some(access) = access {
            windowed.push((prefix.to_string(), access));
        }
    }
    Ok(windowed)
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

pub type ShotNames = Rc<HashSet<String>>;

/// Reads each lane's declarations once for the commits a coverage check needs.
pub struct ShotDeclarations<'a> {
    top: &'a Path,
    names: HashMap<(Lane, String), Option<ShotNames>>,
}

impl<'a> ShotDeclarations<'a> {
    pub fn new(top: &'a Path) -> Self {
        Self {
            top,
            names: HashMap::new(),
        }
    }

    pub fn names(&mut self, lane: Lane, commit: &str) -> Option<ShotNames> {
        self.names
            .entry((lane, commit.to_string()))
            .or_insert_with(|| match declared_names_in_lane(self.top, commit, lane) {
                Ok(names) => Some(Rc::new(names.into_iter().collect())),
                Err(error) => {
                    note(format!(
                        "Builds could not read the {} shots declared at {commit}: {}",
                        lane.word(),
                        error.text()
                    ));
                    None
                }
            })
            .clone()
    }
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
        None => {
            let folders = shots::declared_folders(&config).map_err(Fail::Message)?;
            if folders.is_empty() {
                fail!("{CONFIG} at {commit} has no shots-declared line, so no shot is declared");
            }
            folders
        }
    };
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
