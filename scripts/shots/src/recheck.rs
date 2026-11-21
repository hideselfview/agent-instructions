//! `recheck`: compares again the pairs of a report this checker, or another
//! version of it, wrote, with this checker's comparison. The report holds each
//! pair's two pictures and element lists (report.rs), so the command takes no
//! shot, draws no board and reads no repo: it shows what a change to the
//! comparison clears and adds on a run that has already been made. It prints
//! only what it compared again, the scenes and their findings, and none of
//! the run's failed captures or component listing, which it does not read.

use crate::findings_file::{self, FILE};
use crate::names::Name;
use crate::report::{board_list, board_picture, shot_list, shot_picture};
use crate::shots::{ElementList, Picture, Shot};
use crate::verify::{self, Drawn};
use serde::Deserialize;
use std::path::Path;

/// The parts of the report's findings.json that name what to compare.
#[derive(Deserialize)]
struct Named {
    commit: String,
    scenes: Vec<String>,
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
    Ok(findings_file::rechecked(version, &named.commit, &checked))
}

fn decode(path: &Path) -> Result<Picture, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Picture::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "recheck_tests.rs"]
mod tests;
