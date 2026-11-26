//! `builds shots picture <repo> <ref> <scene> board|shot`: the picture of a
//! scene's board or shot at a commit. An agent never runs the board drawer or
//! the shots program itself, so it asks builds, which hands back the picture
//! from the pictures a cached run of that commit holds, and makes a run only
//! when none holds it. The run is `builds app-build <repo> <ref> --shots
//! <scene>`, which takes its turn in the lane's queue and draws only the
//! boards, and shoots only the app trees, that the repo's picture folder lacks
//! (`builds help`, APP BUILDS).

use std::path::PathBuf;

use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::places::Places;
use crate::shots_cache;
use crate::shots_program::{VERSIONS, Versions};
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds shots picture <repo> <ref> <scene> board|shot";

/// The side of a scene's pair a picture shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// What the board draws.
    Board,
    /// What the app shows.
    Shot,
}

impl Side {
    fn parse(word: &str) -> Result<Self> {
        match word {
            "board" => Ok(Side::Board),
            "shot" => Ok(Side::Shot),
            _ => fail!("{word:?} is not a side of a scene (board | shot); {USAGE}"),
        }
    }

    fn word(self) -> &'static str {
        match self {
            Side::Board => "board",
            Side::Shot => "shot",
        }
    }
}

/// The lane that takes the shots of <scene>, named by the platform its name
/// starts with.
fn lane_of(scene: &str) -> Result<Lane> {
    shots::Prefix::parse(scene).map_err(|error| crate::fail::Fail::Message(error.to_string()))?;
    let (platform, _) = scene.split_once('.').expect("a parsed scene holds a dot");
    match Lane::parse(platform) {
        Some(lane) => Ok(lane),
        None => fail!("builds takes no shots of {platform}, only of mac and android"),
    }
}

/// The picture a run's report keeps for the pair of <scene>, in the report's
/// folder (the shots checker's report.rs names it).
fn file_in_report(scene: &str, side: Side) -> String {
    format!("img/{scene}.{}.png", side.word())
}

/// The picture of <scene>'s <side> that a cached run of <commit> on <lane>,
/// made by the shots program <versions> in any environment, holds: a run of
/// any prefix that starts <scene> covers it. Reading it marks the run used.
fn stored(
    places: &Places,
    repo: &str,
    commit: &str,
    lane: Lane,
    scene: &str,
    side: Side,
    versions: &Versions,
) -> Result<Option<PathBuf>> {
    shots_cache::read_covering(places, repo, commit, lane, scene, versions, |cached| {
        let picture = cached.report().join(file_in_report(scene, side));
        Ok(picture.is_file().then_some(picture))
    })
}

/// The arguments of `builds app-build` that make a run holding <scene> at
/// <git_ref>: --fresh, because a cached run that covers the scene but holds no
/// picture of it would otherwise be printed in place of a new run.
fn app_build_words(repo: &str, git_ref: &str, scene: &str) -> Vec<String> {
    [repo, git_ref, "--shots", scene, "--fresh"]
        .map(str::to_string)
        .to_vec()
}

/// `builds shots picture`, given the arguments after `picture`.
pub fn cmd(tool: &Tool, args: &[String]) -> Result<()> {
    let [repo, git_ref, scene, side] = args else {
        fail!("{USAGE}");
    };
    let (lane, side) = (lane_of(scene)?, Side::parse(side)?);
    let commit = crate::shot_findings::commit_of(&tool.places, repo, git_ref)?;
    let mut found = stored(&tool.places, repo, &commit, lane, scene, side, &VERSIONS)?;
    if found.is_none() {
        note(format!(
            "no cached run of {git_ref} holds the {} of {scene}; asking for one with `builds app-build {repo} {git_ref} --shots {scene}`",
            side.word()
        ));
        let result = crate::app_build::cmd_app_build(tool, &app_build_words(repo, git_ref, scene));
        found = stored(&tool.places, repo, &commit, lane, scene, side, &VERSIONS)?;
        if found.is_none() {
            match result {
                Err(error) => fail!(
                    "the app build that makes the {} of {scene} failed: {}",
                    side.word(),
                    error.text()
                ),
                Ok(()) => fail!(
                    "the app build of {git_ref} made no {} of {scene}: the commit declares no such scene, or the run took no picture of it",
                    side.word()
                ),
            }
        }
    }
    let picture = found.expect("a picture was found or the command failed");
    println!("{}", picture.display());
    Ok(())
}

#[cfg(test)]
#[path = "shots_picture_tests.rs"]
mod tests;
