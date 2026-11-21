//! `recheck`: compares again the pairs of a report this checker, or another
//! version of it, wrote, with this checker's comparison. The report holds each
//! pair's two pictures and element lists (report.rs), so the command takes no
//! shot, draws no board and reads no repo: it shows what a change to the
//! comparison clears and adds on a run that has already been made.

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

/// The findings.json that this checker, version <version>, makes of the pairs
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
        let load = |path: String| {
            let path = report.join(path);
            if path.is_file() {
                Ok(path)
            } else {
                Err(format!(
                    "the report holds no {}: the run that made it kept no element lists",
                    path.display()
                ))
            }
        };
        let board_png = load(board_picture(scene))?;
        let shot_png = load(shot_picture(scene))?;
        let board = Drawn {
            list: ElementList::load(&load(board_list(scene))?).map_err(|e| e.to_string())?,
            picture: decode(&board_png)?,
            png: board_png,
        };
        let shot = Shot {
            list: ElementList::load(&load(shot_list(scene))?).map_err(|e| e.to_string())?,
            picture: decode(&shot_png)?,
        };
        checked.push(verify::compare_pair(&name, board, &shot, shot_png));
    }
    Ok(findings_file::text(
        version,
        &named.commit,
        &checked,
        &[],
        &Default::default(),
    ))
}

fn decode(path: &Path) -> Result<Picture, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Picture::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "recheck_tests.rs"]
mod tests;
