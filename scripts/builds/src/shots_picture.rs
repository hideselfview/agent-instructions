//! `builds shots picture <repo> <ref> <scene> board|shot [--project P]`: the
//! picture of a scene's board or shot at a commit. An agent never runs the board drawer or
//! the shots program itself, so it asks builds, which hands back the picture
//! from the pictures a cached run of that commit holds, and makes a run only
//! when none holds it. The run is `builds app-build <repo> <ref> --shots
//! <scene>`, which takes its turn in the lane's queue and draws only the
//! boards, and shoots only the app trees, that the repo's picture folder lacks
//! (`builds help`, APP BUILDS).

use crate::daemon::component_shots::{Side, picture_file};
use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds shots picture <repo> <ref> <scene> board|shot [--project P]";

/// The lane of <repo> that takes the shots of <scene>, named by the app its
/// name starts with: a mac, android or web app.
fn lane_of(places: &crate::places::Places, repo: &str, scene: &str) -> Result<Lane> {
    shots::Prefix::parse(scene).map_err(|error| crate::fail::Fail::Message(error.to_string()))?;
    let (app, _) = scene.split_once('.').expect("a parsed scene holds a dot");
    crate::repo_lanes::parse(places, repo, app)
}

/// The arguments of `builds app-build` that make a run holding <scene> at
/// <git_ref>: --fresh, because a cached run that covers the scene but holds no
/// picture of it would otherwise be printed in place of a new run.
/// <project> is the project that records the run (`app-build --project`).
fn app_build_words(repo: &str, git_ref: &str, scene: &str, project: Option<&str>) -> Vec<String> {
    [repo, git_ref, "--shots", scene, "--fresh"]
        .into_iter()
        .chain(
            project
                .into_iter()
                .flat_map(|name| [crate::app_build::PROJECT, name]),
        )
        .map(str::to_string)
        .collect()
}

/// `builds shots picture`, given the arguments after `picture`.
pub fn cmd(tool: &Tool, args: &[String]) -> Result<()> {
    let (repo, git_ref, scene, side, project) = match args {
        [repo, git_ref, scene, side] => (repo, git_ref, scene, side, None),
        [repo, git_ref, scene, side, flag, name] if flag == crate::app_build::PROJECT => {
            (repo, git_ref, scene, side, Some(name.as_str()))
        }
        _ => fail!("{USAGE}"),
    };
    let Some(side) = Side::parse(side) else {
        fail!("{side:?} is not a side of a scene (board | shot); {USAGE}");
    };
    let lane = lane_of(&tool.places, repo, scene)?;
    let commit = crate::shot_findings::commit_of(&tool.places, repo, git_ref)?;
    let find = || picture_file(&tool.places, repo, &commit, lane, scene, side);
    let picture = match find()? {
        Some(picture) => picture,
        None => {
            note(format!(
                "no cached run of {git_ref} holds the {side} of {scene}; asking for one with `builds app-build {repo} {git_ref} --shots {scene}`",
                side = side.word()
            ));
            let words = app_build_words(repo, git_ref, scene, project);
            let ended = crate::app_build::cmd_app_build(tool, &words)
                .err()
                .map(|error| error.text());
            match (find()?, ended) {
                (Some(picture), None) => picture,
                (Some(picture), Some(text)) => {
                    note(format!("the app build that made it ended with: {text}"));
                    picture
                }
                (None, Some(text)) => fail!(
                    "the app build that makes the {} of {scene} failed: {text}",
                    side.word()
                ),
                (None, None) => fail!(
                    "the app build of {git_ref} made no {} of {scene}: the commit declares no such scene, or the run took no picture of it",
                    side.word()
                ),
            }
        }
    };
    println!("{}", picture.display());
    Ok(())
}

#[cfg(test)]
#[path = "shots_picture_tests.rs"]
mod tests;
