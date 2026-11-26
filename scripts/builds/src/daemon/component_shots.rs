//! The dashboard's page of one design-system component's shots
//! (`/designs/<project>/component/<Component>`): for the commit origin/main
//! names, each row of the component's preview with its board beside the
//! app's shot of that state, and the findings under them, on each platform
//! that draws the component.
//!
//! The page reads the cached runs (shots_cache.rs): on each lane, the
//! newest run of the commit by this builds' shots version whose prefix
//! starts the component's (`mac.component.checkbox.`, so a run of
//! `mac.component.` or `mac.` serves too). The run's findings.json lists the
//! component's rows in the preview's order and the size of each pair it
//! checked, and its report holds the pair's pictures. When builds has no
//! cached run on a lane, the page asks this Mac's daemon to take the
//! component's shots there (`start`): one `builds app-build <repo> <commit>
//! --shots <prefix>` per lane (runs.rs), whose progress the page shows until
//! the run is cached.
//!
//! The component's name gives its platform and the prefix of its shots
//! (`boards::component_key`): a component of one platform has that
//! platform's lane, and one both platforms share has both lanes. builds
//! caches no web shots, so a web component has no lane.

use std::path::Path;

use boards::Source;
use boards::component_key::ComponentKey;
use boards::preview::preview_path;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::runs::{Phase, RunPage, Runs, StartError};
use crate::design_render::{NOW, fill};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::shots_cache::{self, Cached, Made};
use crate::shots_program::VERSIONS;

const PAGE: &str = include_str!("component_shots.html");

/// What the page shows of a component at one commit.
#[derive(Debug, Serialize)]
pub struct Shots {
    pub component: String,
    pub commit: String,
    /// Each lane that draws the component, mac first.
    pub platforms: Vec<Platform>,
}

/// The component's shots on one lane.
#[derive(Debug, Serialize)]
pub struct Platform {
    pub lane: String,
    /// The start of the names of the component's shots on the lane.
    pub prefix: String,
    /// The cached run that holds them; none when builds has none cached.
    pub run: Option<Run>,
    /// The shots run the dashboard started for the prefix, while it runs or
    /// once it ended for this commit.
    pub capture: Option<RunPage>,
}

/// A cached run, as the page shows the component's part of it.
#[derive(Debug, Serialize)]
pub struct Run {
    /// The prefix the run took.
    pub prefix: String,
    /// When the run ended, seconds since 1970.
    pub recorded_at: i64,
    /// Each preview row, in the preview's order.
    pub rows: Vec<Row>,
}

/// A preview row and the run's pair of it.
#[derive(Debug, Serialize, PartialEq)]
pub struct Row {
    pub shot: String,
    /// The row's state: the shot's name after the component's prefix.
    pub state: String,
    /// The pair's board and shot sizes in points; none when the run checked
    /// no pair of the row.
    pub sizes: Option<Sizes>,
    pub findings: Vec<Said>,
}

/// A pair's board and shot sizes, in points, as findings.json gives them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sizes {
    board: Size,
    shot: Size,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
struct Size {
    width: f64,
    height: f64,
}

/// A finding or warning of a pair: its level and sentence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Said {
    level: String,
    sentence: String,
}

/// The parts of a run's findings.json that the page reads.
#[derive(Deserialize)]
struct File {
    sizes: std::collections::BTreeMap<String, Sizes>,
    components: Vec<Listed>,
    findings: Vec<Record>,
}

/// A component on one platform and its rows' shots, as findings.json lists
/// it.
#[derive(Deserialize)]
struct Listed {
    name: String,
    platform: String,
    rows: Vec<String>,
}

#[derive(Deserialize)]
struct Record {
    scene: String,
    #[serde(flatten)]
    said: Said,
}

/// Which picture of a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Board,
    Shot,
}

impl Side {
    /// The side named <word> in a picture's address (`board`, `shot`).
    pub fn parse(word: &str) -> Option<Side> {
        match word {
            "board" => Some(Side::Board),
            "shot" => Some(Side::Shot),
            _ => None,
        }
    }

    /// The side's word in a picture's address.
    pub fn word(self) -> &'static str {
        match self {
            Side::Board => "board",
            Side::Shot => "shot",
        }
    }

    /// The picture file the report of a run keeps for <scene>'s side, in the
    /// report's folder.
    fn file(self, scene: &str) -> String {
        match self {
            Side::Board => shots::board_picture(scene),
            Side::Shot => shots::shot_picture(scene),
        }
    }
}

/// The lanes among <repo_lanes>, a repo's apps, that draw component
/// <component>, each with the prefix of its shots there: every lane for a
/// component all platforms share, else the lanes on the component's platform.
/// Fails when <component> is no component's name.
pub fn lanes(repo_lanes: &[Lane], component: &str) -> Result<Vec<(Lane, String)>> {
    let Some(key) = ComponentKey::of(component) else {
        fail!("{component:?} is no component's name: a capital, then letters and digits");
    };
    Ok(repo_lanes
        .iter()
        .copied()
        .filter(|lane| {
            key.platform
                .is_none_or(|word| word == lane.platform().word())
        })
        .map(|lane| (lane, key.prefix(lane.word())))
        .collect())
}

/// What the page shows of <component> at <repo>'s <commit>: on each of its
/// lanes, the cached run and the shots run <capture> gives for the
/// lane and prefix, if it runs or ended for <commit>. Reading a run marks it
/// used.
pub fn shots(
    places: &Places,
    repo: &str,
    commit: &str,
    component: &str,
    repo_lanes: &[Lane],
    capture: &dyn Fn(Lane, &str) -> Option<RunPage>,
) -> Result<Shots> {
    let lanes = lanes(repo_lanes, component)?;
    let mut platforms = Vec::new();
    for (lane, prefix) in lanes {
        let run =
            shots_cache::read_covering(places, repo, commit, lane, &prefix, &VERSIONS, |cached| {
                read_run(cached, component, lane, &prefix).map(Some)
            })?;
        let capture = capture(lane, &prefix).filter(|run| {
            run.reference.as_deref() == Some(commit)
                || matches!(run.phase, Phase::Queued | Phase::Running | Phase::Detached)
        });
        platforms.push(Platform {
            lane: lane.word().to_string(),
            prefix,
            run,
            capture,
        });
    }
    Ok(Shots {
        component: component.to_string(),
        commit: commit.to_string(),
        platforms,
    })
}

/// <component>'s rows on <lane> in the cached run <cached>: the rows its
/// findings.json lists for the component, with the pairs it checked. A run by
/// this shots version lists each component with a preview, so a run that
/// lists none of <component>, or a row of it whose shot does not start with
/// <prefix>, fails the read.
fn read_run(cached: &Cached, component: &str, lane: Lane, prefix: &str) -> Result<Run> {
    let path = cached.findings_json();
    let text = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    let file: File = serde_json::from_str(&text)
        .map_err(|error| Fail::Message(format!("{}: {error}", path.display())))?;
    let Some(listed) = file
        .components
        .iter()
        .find(|listed| listed.name == component && listed.platform == lane.word())
    else {
        fail!(
            "{} lists no preview rows of {component} on {}, and the commit has its preview",
            path.display(),
            lane.word()
        );
    };
    let mut rows = Vec::new();
    for shot in &listed.rows {
        let Some(state) = shot.strip_prefix(prefix) else {
            fail!(
                "{} lists {shot} as a row of {component}, whose shots start with {prefix}",
                path.display()
            );
        };
        rows.push(Row {
            shot: shot.clone(),
            state: state.to_string(),
            sizes: file.sizes.get(shot).cloned(),
            findings: file
                .findings
                .iter()
                .filter(|record| &record.scene == shot)
                .map(|record| record.said.clone())
                .collect(),
        });
    }
    Ok(Run {
        prefix: cached.key.prefix.clone(),
        recorded_at: cached.recorded_at(),
        rows,
    })
}

/// Takes <component>'s shots of <repo>'s <commit> on each of its lanes where
/// builds has no cached run of them and no shots run of its prefix runs (runs.rs).
pub fn start(
    places: &Places,
    runs: &std::sync::Arc<Runs>,
    repo: &str,
    commit: &str,
    component: &str,
    repo_lanes: &[Lane],
) -> Result<()> {
    for (lane, prefix) in lanes(repo_lanes, component)? {
        let cached =
            shots_cache::read_covering(places, repo, commit, lane, &prefix, &VERSIONS, |_| {
                Ok(Some(()))
            })?;
        if cached.is_some() {
            continue;
        }
        match runs.start_shots(repo, lane, commit, &prefix) {
            Ok(()) | Err(StartError::Busy(_)) => {}
            Err(error) => fail!("{}", error.message()),
        }
    }
    Ok(())
}

/// The <side> picture of the pair <scene> in the cached run of
/// <repo>'s <commit> on <lane>; none when it has no cached run or picture of it.
/// Fails when <scene> is no shot's name.
pub fn picture(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    scene: &str,
    side: Side,
) -> Result<Option<Vec<u8>>> {
    let Some(file) = picture_file(places, repo, commit, lane, scene, side)? else {
        return Ok(None);
    };
    match std::fs::read(&file) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => fail!("reading {}: {error}", file.display()),
    }
}

/// The file of the picture `picture` reads: the report of the cached run of
/// <repo>'s <commit> on <lane> that covers <scene> keeps one of its <side>;
/// none when no such run holds it. Reading marks the run used.
pub fn picture_file(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    scene: &str,
    side: Side,
) -> Result<Option<std::path::PathBuf>> {
    let plain = !scene.is_empty()
        && !scene.contains("..")
        && scene
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
    if !plain {
        fail!("{scene:?} is no shot's name");
    }
    if !scene.starts_with(&format!("{}.", lane.word())) {
        return Ok(None);
    }
    shots_cache::read_covering(places, repo, commit, lane, scene, &VERSIONS, |cached| {
        let file = cached.report().join(side.file(scene));
        match std::fs::metadata(&file) {
            Ok(found) => Ok(found.is_file().then_some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => fail!("reading {}: {error}", file.display()),
        }
    })
}

/// The page of <component>'s shots in project <project>, whose design files
/// are in <repo> (named <repo_name>), at the commit origin/main names there.
/// Opening the page marks each cached run it shows used. <here> says whether
/// the page is asked for on this Mac, where it can start a shots run.
pub fn page(
    places: &Places,
    project: &str,
    repo_name: &str,
    repo: &Path,
    component: &str,
    here: bool,
) -> Result<String> {
    let lanes = lanes(&crate::repo_lanes::in_source(repo)?, component)?;
    let commit = Source::Commit { repo, rev: NOW }
        .check()?
        .expect("a commit source names a commit");
    let from = Source::Commit { repo, rev: &commit };
    if !from.has(&preview_path(component))? {
        fail!("commit {commit} has no {}", preview_path(component));
    }
    if !lanes.is_empty() {
        let _reading = shots_cache::writing(places)?;
        for (lane, prefix) in &lanes {
            shots_cache::covering_run_used(
                places,
                repo_name,
                &commit,
                *lane,
                Made::Anywhere,
                prefix,
                &VERSIONS,
            )?;
        }
    }
    let data = json!({
        "project": project,
        "component": component,
        "commit": commit,
        "here": here,
        "designs": format!("/designs/{project}"),
        "base": format!("/designs/{project}/{commit}"),
    });
    // `<` never appears in the JSON as itself, so no text can end the script.
    let data = data.to_string().replace('<', "\\u003c");
    Ok(fill(
        PAGE,
        &[
            ("PALETTE", super::designs::PALETTE),
            ("CONTROLS", super::designs::CONTROLS),
            ("DATA", &data),
        ],
    ))
}

#[cfg(test)]
#[path = "component_shots_tests.rs"]
mod tests;
