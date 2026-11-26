//! `recheck`: compares again the pairs of a report this checker, or another
//! version of it, wrote, with this checker's comparison. The report holds each
//! pair's two pictures and element lists (report.rs), so the command takes no
//! shot, draws no board and reads no repo: it shows what a change to the
//! comparison clears and adds on a run that has already been made. It prints
//! only what it compared again, the scenes and their findings, and none of
//! the run's failed captures or component listing, which it does not read.

use crate::capture::Failed;
use crate::findings_file::{self, FILE, FailedScene};
use crate::names::{App, Name, NameError, Platform};
use crate::report::{board_list, board_picture, shot_list, shot_picture};
use crate::shots::{ElementList, Picture, Shot};
use crate::verify::{self, Drawn, Outcome};
use serde::Deserialize;
use std::path::Path;

/// The parts of the report's findings.json that name what to compare.
#[derive(Deserialize)]
struct Named {
    /// The capture version of the checker that made the report's pictures,
    /// which a comparison again leaves as it is.
    capture_version: String,
    commit: String,
    scenes: Vec<String>,
    /// The board hash of each scene's pair, which a comparison again leaves
    /// as it is.
    boards: std::collections::BTreeMap<String, String>,
    /// The scenes the capture wrote no usable shot of, which fail the run.
    failed: Vec<FailedScene>,
    /// How many problems the run found, each of which fails it.
    problems: usize,
}

/// The scenes and findings (findings_file.rs `rechecked`) that this checker, version <version>, makes of the pairs
/// the report in <report> holds.
pub(crate) fn text(report: &Path, version: &str) -> Result<String, String> {
    let file = report.join(FILE);
    let named: Named = serde_json::from_str(
        &std::fs::read_to_string(&file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?,
    )
    .map_err(|e| format!("{}: {e}", file.display()))?;
    let mut checked = Vec::new();
    for scene in &named.scenes {
        let board_png = report.join(board_picture(scene));
        let shot_png = report.join(shot_picture(scene));
        let board = Drawn {
            list: ElementList::load(&report.join(board_list(scene))).map_err(|e| e.to_string())?,
            picture: decode(&board_png)?,
            png: board_png,
            hash: named
                .boards
                .get(scene)
                .ok_or_else(|| format!("{}: names no board hash for {scene}", file.display()))?
                .clone(),
        };
        let shot = Shot {
            list: ElementList::load(&report.join(shot_list(scene))).map_err(|e| e.to_string())?,
            picture: decode(&shot_png)?,
        };
        // The report reads no repo, so the shot's own list gives the platform
        // of the app the scene's first segment names.
        let name = scene_name(scene, shot.list.platform)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        checked.push(verify::compare_pair(&name, board, &shot, shot_png));
    }
    let mut failed = Vec::new();
    for scene in &named.failed {
        let platform = crate::backend::from_file_word(&scene.platform).ok_or_else(|| {
            format!(
                "{}: {} names the platform word {:?}, which is no platform's",
                file.display(),
                scene.scene,
                scene.platform
            )
        })?;
        let name =
            scene_name(&scene.scene, platform).map_err(|e| format!("{}: {e}", file.display()))?;
        failed.push(Failed {
            name,
            why: scene.error.clone(),
        });
    }
    // The run has no before run, so each pair's findings fail it, as the
    // checker's own verdict says (`Outcome::passed`). The report holds only
    // the count of the run's problems, so each counts as one that fails.
    let outcome = Outcome {
        runtime: String::new(),
        checked,
        problems: Vec::new(),
        lists: Default::default(),
        not_taken: Vec::new(),
        failed,
        components: Default::default(),
    };
    Ok(findings_file::rechecked(
        version,
        &named.capture_version,
        &named.commit,
        &outcome.checked,
        named.problems == 0 && outcome.passed(),
    ))
}

/// The name of the scene <scene>, of the app its first segment names, which
/// runs on <platform>.
fn scene_name(scene: &str, platform: Platform) -> Result<Name, NameError> {
    let app = scene.split('.').next().unwrap_or_default();
    Name::parse_in(
        scene,
        &App {
            name: app.to_string(),
            platform,
        },
    )
}

fn decode(path: &Path) -> Result<Picture, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Picture::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "recheck_tests.rs"]
mod tests;
