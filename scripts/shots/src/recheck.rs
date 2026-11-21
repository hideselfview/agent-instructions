//! `recheck`: compares again the pairs of a report this checker, or another
//! version of it, wrote, with this checker's comparison. The report holds each
//! pair's two pictures and element lists (report.rs), so the command takes no
//! shot, draws no board and reads no repo: it shows what a change to the
//! comparison clears and adds on a run that has already been made. It prints
//! only what it compared again, the scenes and their findings, and none of
//! the run's failed captures or component listing, which it does not read.

use crate::capture::Failed;
use crate::findings_file::{self, FILE};
use crate::names::Name;
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
    /// The scenes the capture wrote no usable shot of, which fail the run.
    failed: Vec<FailedScene>,
}

/// A scene of a report's findings.json that the capture failed.
#[derive(Deserialize)]
struct FailedScene {
    scene: String,
    error: String,
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
        let name = Name::parse(scene).map_err(|e| format!("{}: {e}", file.display()))?;
        let board_png = report.join(board_picture(scene));
        let shot_png = report.join(shot_picture(scene));
        let board = Drawn {
            list: ElementList::load(&report.join(board_list(scene))).map_err(|e| e.to_string())?,
            picture: decode(&board_png)?,
            png: board_png,
        };
        let shot = Shot {
            list: ElementList::load(&report.join(shot_list(scene))).map_err(|e| e.to_string())?,
            picture: decode(&shot_png)?,
        };
        checked.push(verify::compare_pair(&name, board, &shot, shot_png));
    }
    let mut failed = Vec::new();
    for scene in &named.failed {
        let name = Name::parse(&scene.scene).map_err(|e| format!("{}: {e}", file.display()))?;
        failed.push(Failed {
            name,
            why: scene.error.clone(),
        });
    }
    // The run has no before run, so each pair's findings fail it, as the
    // checker's own verdict says (`Outcome::passed`).
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
        outcome.passed(),
    ))
}

fn decode(path: &Path) -> Result<Picture, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Picture::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "recheck_tests.rs"]
mod tests;
