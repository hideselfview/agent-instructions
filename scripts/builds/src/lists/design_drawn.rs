//! Whether a design change's boards and component previews draw the same at
//! two commits. The person approves what boards draw, so two commits hold
//! the same drawing when each board and preview of the change comes out
//! alike, whatever their files' bytes: a rebase that merges unrelated lines
//! into a board file, `canvas.json` or a component's source changes bytes and
//! no drawing, and keeps the approval.
//!
//! `Sameness::between` compares two commits' files first (`Drawing`); when
//! any file differs, it asks the shots checker (`builds shots draws`, which
//! draws each board and preview at both commits in its pinned Chrome and
//! compares the picture and the element boxes) about the change's places. A
//! board or component place is the same when its own drawing is alike. A
//! shared file (`canvas.json`, a design-system source) is the same when every
//! place of the change draws alike, because such a file draws nothing alone.
//! A change that holds no board or preview has no drawing to compare, so a
//! shared file that differs is never the same for it. A repo whose
//! canvas.json names no design system has no boards the checker can draw, so
//! its files' bytes decide and nothing is drawn.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::design_paths::{Drawing, PlaceKind};

/// Draws boards and component previews at two commits.
pub trait Renders: Send + Sync {
    /// The places among <places> (board files and component folders) that
    /// draw alike at commits <before> and <after> of the checkout at <top>.
    fn alike(
        &self,
        top: &Path,
        before: &str,
        after: &str,
        places: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>>;
}

/// Renders by running `builds shots draws` of the builds program at this
/// path.
pub struct ProgramRenders(pub PathBuf);

/// What `builds shots draws` is asked for a place: its flag and its name.
fn asked(place: &str) -> Result<(&'static str, String)> {
    Ok(match PlaceKind::of(place) {
        PlaceKind::Board => ("--board", boards::board_name(place)?),
        PlaceKind::Component => (
            "--component",
            place
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string(),
        ),
        PlaceKind::CanvasIndex | PlaceKind::Spec => {
            unreachable!("only boards and components are drawn")
        }
    })
}

impl Renders for ProgramRenders {
    fn alike(
        &self,
        top: &Path,
        before: &str,
        after: &str,
        places: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>> {
        let mut command = Command::new(&self.0);
        command.args(["shots", "draws", "--at", before, "--against", after]);
        let mut lines = BTreeMap::new();
        for place in places {
            let (flag, name) = asked(place)?;
            let what = if flag == "--board" {
                "board"
            } else {
                "component"
            };
            lines.insert(format!("{what} {name}"), place.clone());
            command.args([flag, &name]);
        }
        let ran = command
            .current_dir(top)
            .stdin(std::process::Stdio::null())
            .output()
            .context(|| format!("running {} shots draws", self.0.display()))?;
        if !ran.status.success() {
            fail!(
                "the shots checker could not draw the boards at {before} and {after} ({}):\n{}",
                ran.status,
                String::from_utf8_lossy(&ran.stderr).trim_end()
            );
        }
        let answered = String::from_utf8_lossy(&ran.stdout);
        let mut alike = BTreeSet::new();
        let mut told = BTreeSet::new();
        for line in answered.lines() {
            let Some((verdict, what)) = line.split_once(' ') else {
                fail!("the shots checker answered {line:?}, which is no `alike` or `differs` line");
            };
            let Some(place) = lines.get(what) else {
                fail!("the shots checker answered for {what:?}, which builds did not ask about");
            };
            match verdict {
                "alike" => {
                    alike.insert(place.clone());
                }
                "differs" => {}
                _ => fail!(
                    "the shots checker answered {line:?}, which is no `alike` or `differs` line"
                ),
            }
            told.insert(place.clone());
        }
        if let Some(silent) = lines.values().find(|place| !told.contains(*place)) {
            fail!("the shots checker did not say whether {silent} draws alike");
        }
        Ok(alike)
    }
}

/// Renders no board alike, so the files' bytes alone decide.
#[cfg(test)]
pub struct NeverAlike;

#[cfg(test)]
impl Renders for NeverAlike {
    fn alike(&self, _: &Path, _: &str, _: &str, _: &BTreeSet<String>) -> Result<BTreeSet<String>> {
        Ok(BTreeSet::new())
    }
}

/// Does the canvas.json at commit <rev> of the checkout at <top> name the
/// design system the boards load? A repo whose canvas names none (or has no
/// canvas.json) has no boards the shots checker can draw, so its drawings are
/// judged by their files' bytes alone.
fn names_design_system(top: &Path, rev: &str) -> Result<bool> {
    if !boards::exists_in(top, rev, boards::CANVAS)? {
        return Ok(false);
    }
    let text = boards::git_text(
        boards::git(top).args(["show", &format!("{rev}:{}", boards::CANVAS)]),
        &format!("read {} at {rev}", boards::CANVAS),
    )?;
    let canvas: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        crate::fail::Fail::Message(format!("{} at {rev} is not JSON: {e}", boards::CANVAS))
    })?;
    let named = canvas["designSystems"].as_array().map_or(0, Vec::len);
    if named == 0 {
        return Ok(false);
    }
    // Two systems, or a namespace that is no folder name, is a broken canvas.
    boards::design_namespace(&canvas).map_err(|e| {
        crate::fail::Fail::Message(format!("{} at {rev}: {}", boards::CANVAS, e.text()))
    })?;
    Ok(true)
}

/// Whether each path a design change shows draws the same at two commits.
#[derive(Debug, Default)]
pub struct Sameness {
    /// The paths whose files hold the same bytes at both commits.
    identical: BTreeSet<String>,
    /// Each board or component place the change holds, and whether it draws
    /// alike; empty when every file was identical, so nothing was drawn.
    drawn: BTreeMap<String, bool>,
    /// The design-system sources among the paths compared: shared files that
    /// are judged by every place of the change.
    sources: BTreeSet<String>,
}

impl Sameness {
    /// The sameness of nothing: no path draws the same, as for a change that
    /// has no last commit to compare with.
    pub fn never() -> Sameness {
        Sameness::default()
    }

    /// How <paths> draw at commit <before> (`before.1` its drawing) and at
    /// commit <after>, for a change that holds the places <held>. <sources>
    /// are the shared design-system sources among <paths>. Asks <renders>
    /// only when some file of <paths> differs.
    pub fn between(
        renders: &dyn Renders,
        top: &Path,
        before: (&str, &Drawing),
        after: (&str, &Drawing),
        held: &BTreeSet<String>,
        sources: &BTreeSet<String>,
        paths: &BTreeSet<&String>,
    ) -> Result<Sameness> {
        let identical: BTreeSet<String> = paths
            .iter()
            .filter(|path| before.1.same(after.1, path))
            .map(|path| path.to_string())
            .collect();
        let mut drawn = BTreeMap::new();
        if identical.len() < paths.len()
            && names_design_system(top, before.0)?
            && names_design_system(top, after.0)?
        {
            let places: BTreeSet<String> = held
                .iter()
                .filter(|place| {
                    matches!(
                        PlaceKind::of(place),
                        PlaceKind::Board | PlaceKind::Component
                    )
                })
                .cloned()
                .collect();
            if !places.is_empty() {
                let alike = renders.alike(top, before.0, after.0, &places)?;
                for place in places {
                    let same = alike.contains(&place);
                    drawn.insert(place, same);
                }
            }
        }
        Ok(Sameness {
            identical,
            drawn,
            sources: sources.clone(),
        })
    }

    /// Does <path> draw the same at both commits?
    pub fn path(&self, path: &str) -> bool {
        if self.identical.contains(path) || self.drawn.get(path) == Some(&true) {
            return true;
        }
        let shared = PlaceKind::of(path) == PlaceKind::CanvasIndex || self.sources.contains(path);
        shared && !self.drawn.is_empty() && self.drawn.values().all(|alike| *alike)
    }
}

#[cfg(test)]
#[path = "design_drawn_tests.rs"]
mod tests;
