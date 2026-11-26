//! `builds shots picture <repo> <ref> <scene> board|shot`: the picture of a
//! scene's board or shot at a commit. An agent never runs the board drawer or
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

const USAGE: &str = "usage: builds shots picture <repo> <ref> <scene> board|shot";

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
    let Some(side) = Side::parse(side) else {
        fail!("{side:?} is not a side of a scene (board | shot); {USAGE}");
    };
    let lane = lane_of(scene)?;
    let commit = crate::shot_findings::commit_of(&tool.places, repo, git_ref)?;
    let find = || picture_file(&tool.places, repo, &commit, lane, scene, side);
    let picture = match find()? {
        Some(picture) => picture,
        None => {
            note(format!(
                "no cached run of {git_ref} holds the {side} of {scene}; asking for one with `builds app-build {repo} {git_ref} --shots {scene}`",
                side = side.word()
            ));
            let ended =
                crate::app_build::cmd_app_build(tool, &app_build_words(repo, git_ref, scene))
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
