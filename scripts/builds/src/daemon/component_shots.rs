//! The dashboard's page of one design-system component's shots
//! (`/designs/<project>/component/<Component>`): for the commit origin/main
//! names, each row of the component's preview with its board beside the
//! app's shot of that state, and the findings under them, on each platform
//! that draws the component.
//!
//! The page reads the runs builds keeps (shot_store.rs): on each lane, the
//! newest run of the commit by this builds' shots version whose prefix
//! starts the component's (`mac.component.checkbox.`, so a run of
//! `mac.component.` or `mac.` serves too). The run's findings.json lists the
//! component's rows in the preview's order and the size of each pair it
//! checked, and its report holds the pair's pictures. When builds keeps no
//! such run on a lane, the page asks this Mac's daemon to take the
//! component's shots there (`start`): one `builds app-build <repo> <commit>
//! --shots <prefix>` per lane (runs.rs), whose progress the page shows until
//! the run is kept.
//!
//! The component's name gives its platform and the prefix of its shots
//! (`boards::component_key`): a component of one platform has that
//! platform's lane, and one both platforms share has both lanes. builds
//! keeps no web shots, so a web component has no lane.

use std::path::Path;

use boards::Source;
use boards::component_key::ComponentKey;
use boards::preview::preview_path;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::runs::{Phase, RunPage, Runs, StartError};
use crate::design_render::{NOW, fill};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lane::Lane;
use crate::places::Places;
use crate::shot_store::{self, Stored};
use crate::shots_program::VERSION;

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
    pub lane: &'static str,
    /// The start of the names of the component's shots on the lane.
    pub prefix: String,
    /// The kept run that holds them; none when builds keeps none.
    pub run: Option<Run>,
    /// The shots run the dashboard started for the prefix, while it runs or
    /// once it ended for this commit.
    pub capture: Option<RunPage>,
}

/// A kept run, as the page shows the component's part of it.
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
    scenes: Vec<String>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    fn word(self) -> &'static str {
        match self {
            Side::Board => "board",
            Side::Shot => "shot",
        }
    }
}

/// The lanes that draw component <component>, each with the prefix of its
/// shots there; fails when <component> is no component's name.
pub fn lanes(component: &str) -> Result<Vec<(Lane, String)>> {
    let Some(key) = ComponentKey::of(component) else {
        fail!("{component:?} is no component's name: a capital, then letters and digits");
    };
    let lanes: Vec<Lane> = match key.platform {
        None => Lane::ALL.to_vec(),
        Some(word) => Lane::parse(word).into_iter().collect(),
    };
    Ok(lanes
        .into_iter()
        .map(|lane| (lane, key.prefix(lane.word())))
        .collect())
}

/// What the page shows of <component> at <repo>'s <commit>: on each of its
/// lanes, the run builds keeps and the shots run <capture> gives for the
/// lane and prefix, if it runs or ended for <commit>. Reads write nothing,
/// so the page's stream reads again on each change of the dashboard.
pub fn shots(
    places: &Places,
    repo: &str,
    commit: &str,
    component: &str,
    capture: &dyn Fn(Lane, &str) -> Option<RunPage>,
) -> Result<Shots> {
    let mut platforms = Vec::new();
    for (lane, prefix) in lanes(component)? {
        let run = match shot_store::covering_run(places, repo, commit, lane, &prefix, VERSION)? {
            Some(stored) => Some(read_run(&stored, component, lane, &prefix)?),
            None => None,
        };
        let capture = capture(lane, &prefix).filter(|run| {
            run.reference.as_deref() == Some(commit)
                || matches!(run.phase, Phase::Queued | Phase::Running | Phase::Detached)
        });
        platforms.push(Platform {
            lane: lane.word(),
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

/// <component>'s rows on <lane> in the kept run <stored>: the rows its
/// findings.json lists for the component, with the pairs it checked.
fn read_run(stored: &Stored, component: &str, lane: Lane, prefix: &str) -> Result<Run> {
    let path = stored.findings_json();
    let text = std::fs::read_to_string(&path).context(|| format!("reading {}", path.display()))?;
    let file: File = serde_json::from_str(&text)
        .map_err(|error| Fail::Message(format!("{}: {error}", path.display())))?;
    let listed = file
        .components
        .iter()
        .find(|listed| listed.name == component && listed.platform == lane.word());
    // A component the run lists no preview of has the rows of the pairs it
    // checked.
    let shots: Vec<&String> = match listed {
        Some(listed) => listed.rows.iter().collect(),
        None => file
            .scenes
            .iter()
            .filter(|scene| scene.starts_with(prefix))
            .collect(),
    };
    let rows = shots
        .into_iter()
        .map(|shot| Row {
            shot: shot.clone(),
            state: shot.strip_prefix(prefix).unwrap_or(shot).to_string(),
            sizes: file.sizes.get(shot).cloned(),
            findings: file
                .findings
                .iter()
                .filter(|record| &record.scene == shot)
                .map(|record| record.said.clone())
                .collect(),
        })
        .collect();
    Ok(Run {
        prefix: stored.key.prefix.clone(),
        recorded_at: stored.recorded_at,
        rows,
    })
}

/// Takes <component>'s shots of <repo>'s <commit> on each of its lanes where
/// builds keeps no run of them and no shots run of its prefix runs (runs.rs).
pub fn start(
    places: &Places,
    runs: &std::sync::Arc<Runs>,
    repo: &str,
    commit: &str,
    component: &str,
) -> Result<()> {
    for (lane, prefix) in lanes(component)? {
        if shot_store::covering_run(places, repo, commit, lane, &prefix, VERSION)?.is_some() {
            continue;
        }
        match runs.start_shots(repo, lane, commit, &prefix) {
            Ok(()) | Err(StartError::Busy(_)) => {}
            Err(error) => fail!("{}", error.message()),
        }
    }
    Ok(())
}

/// The <side> picture of the pair <scene> in the run builds keeps of
/// <repo>'s <commit> on <lane>; none when it keeps no run or picture of it.
/// Fails when <scene> is no shot's name.
pub fn picture(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    scene: &str,
    side: Side,
) -> Result<Option<Vec<u8>>> {
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
    let Some(stored) = shot_store::covering_run(places, repo, commit, lane, scene, VERSION)? else {
        return Ok(None);
    };
    let file = stored
        .report()
        .join("img")
        .join(format!("{scene}.{}.png", side.word()));
    match std::fs::read(&file) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => fail!("reading {}: {error}", file.display()),
    }
}

/// The page of <component>'s shots in project <project>, whose design files
/// are in <repo> (named <repo_name>), at the commit origin/main names there.
/// Opening the page marks each kept run it shows used. <here> says whether
/// the page is asked for on this Mac, where it can start a shots run.
pub fn page(
    places: &Places,
    project: &str,
    repo_name: &str,
    repo: &Path,
    component: &str,
    here: bool,
) -> Result<String> {
    let lanes = lanes(component)?;
    let commit = Source::Commit { repo, rev: NOW }
        .check()?
        .expect("a commit source names a commit");
    let from = Source::Commit { repo, rev: &commit };
    if !from.has(&preview_path(component))? {
        fail!("commit {commit} has no {}", preview_path(component));
    }
    for (lane, prefix) in &lanes {
        if let Some(stored) =
            shot_store::covering_run(places, repo_name, &commit, *lane, prefix, VERSION)?
        {
            shot_store::mark_used(&stored)?;
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

/// <shots> as the page's stream sends it.
pub fn shots_json(shots: Result<Shots>) -> Value {
    match shots {
        Ok(shots) => serde_json::to_value(shots).expect("the shots serialize"),
        Err(error) => json!({ "error": error.text() }),
    }
}

#[cfg(test)]
#[path = "component_shots_tests.rs"]
mod tests;
