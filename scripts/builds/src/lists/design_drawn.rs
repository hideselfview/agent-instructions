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

use std::cell::Cell;
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
/// path, and keeps each answer. Whether a place draws alike at two commits
/// depends only on the two commits' files and the shots checker's capture
/// version, so the answer for a place, both full commit ids and the capture
/// version is written once, when the pair is first drawn, and read on every
/// later ask by amend, health, land and the dashboard: drawing a board takes
/// seconds of Chrome, and health asks for every open design change on every
/// run. A key with all of these can never give a stale answer. A drawing that
/// fails keeps nothing.
pub struct ProgramRenders {
    program: PathBuf,
    /// The folder of the kept answers, one small file for each key.
    kept: PathBuf,
}

impl ProgramRenders {
    /// Renders with the builds program at <program>, keeping its answers in
    /// the caches of <places>.
    pub fn of(places: &crate::places::Places, program: PathBuf) -> ProgramRenders {
        ProgramRenders {
            program,
            kept: places.caches().join("design-alike"),
        }
    }

    /// The file that keeps whether <place> draws alike at the commits
    /// <before> and <after>, given by full id.
    fn file(&self, before: &str, after: &str, place: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        let key = format!(
            "{}\n{before}\n{after}\n{place}",
            crate::shots_program::VERSIONS.capture
        );
        let digest = crate::text::hex(&Sha256::digest(key.as_bytes()));
        self.kept.join(digest)
    }

    /// Keeps <alike> as the answer in <file>.
    fn keep(&self, file: &Path, alike: bool) -> Result<()> {
        std::fs::create_dir_all(&self.kept)
            .context(|| format!("making {}", self.kept.display()))?;
        crate::whole_file::write(file, if alike { "alike" } else { "differs" })
    }
}

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
        let full_before = crate::lists::design_paths::full_sha(top, before)?;
        let full_after = crate::lists::design_paths::full_sha(top, after)?;
        let mut alike = BTreeSet::new();
        let mut undrawn = BTreeSet::new();
        for place in places {
            match std::fs::read_to_string(self.file(&full_before, &full_after, place)) {
                Ok(answer) if answer == "alike" => {
                    alike.insert(place.clone());
                }
                Ok(answer) if answer == "differs" => {}
                Ok(answer) => fail!("the kept answer for {place} says {answer:?}"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    undrawn.insert(place.clone());
                }
                Err(error) => fail!("reading the kept answer for {place}: {error}"),
            }
        }
        if !undrawn.is_empty() {
            let drawn = self.draw(top, &full_before, &full_after, &undrawn)?;
            for place in &undrawn {
                let same = drawn.contains(place);
                self.keep(&self.file(&full_before, &full_after, place), same)?;
                if same {
                    alike.insert(place.clone());
                }
            }
        }
        Ok(alike)
    }
}

impl ProgramRenders {
    /// Draws <places> at the two commits with the builds program, keeping
    /// nothing.
    fn draw(
        &self,
        top: &Path,
        before: &str,
        after: &str,
        places: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>> {
        let mut command = Command::new(&self.program);
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
            .context(|| format!("running {} shots draws", self.program.display()))?;
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
    let not_a_canvas = |e: serde_json::Error| {
        crate::fail::Fail::Message(format!("{} at {rev} is not a canvas: {e}", boards::CANVAS))
    };
    // serde reads a JSON list as a struct's fields in order, so the canvas is
    // read as an object first.
    let mut fields: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&text).map_err(not_a_canvas)?;
    let systems: Vec<System> = match fields.remove("designSystems") {
        None => Vec::new(),
        Some(listed) => serde_json::from_value(listed).map_err(not_a_canvas)?,
    };
    if systems.is_empty() {
        return Ok(false);
    }
    // Two systems, or a namespace that is no folder name, is a broken canvas.
    let named = serde_json::json!({ "designSystems": systems });
    boards::design_namespace(&named).map_err(|e| {
        crate::fail::Fail::Message(format!("{} at {rev}: {}", boards::CANVAS, e.text()))
    })?;
    Ok(true)
}

/// One entry of canvas.json's `designSystems`: serde refuses an entry with no
/// namespace and a list that holds anything but entries.
#[derive(serde::Deserialize, serde::Serialize)]
struct System {
    namespace: String,
}

/// The files at a comparison tip, with its canvas availability read on demand.
pub(crate) struct TipDrawing<'a> {
    rev: &'a str,
    files: &'a Drawing,
    has_system: Cell<Option<bool>>,
}

impl<'a> TipDrawing<'a> {
    pub(crate) fn new(rev: &'a str, files: &'a Drawing) -> Self {
        Self {
            rev,
            files,
            has_system: Cell::new(None),
        }
    }

    fn names_design_system(&self, top: &Path) -> Result<bool> {
        match self.has_system.get() {
            Some(has_system) => Ok(has_system),
            None => {
                let has_system = names_design_system(top, self.rev)?;
                self.has_system.set(Some(has_system));
                Ok(has_system)
            }
        }
    }
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
    pub(crate) fn between(
        renders: &dyn Renders,
        top: &Path,
        before: (&str, &Drawing),
        after: &TipDrawing,
        held: &BTreeSet<String>,
        sources: &BTreeSet<String>,
        paths: &BTreeSet<&String>,
    ) -> Result<Sameness> {
        let identical: BTreeSet<String> = paths
            .iter()
            .filter(|path| before.1.same(after.files, path))
            .map(|path| path.to_string())
            .collect();
        let mut drawn = BTreeMap::new();
        if identical.len() < paths.len()
            && names_design_system(top, before.0)?
            && after.names_design_system(top)?
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
                let alike = renders.alike(top, before.0, after.rev, &places)?;
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
