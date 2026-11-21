//! Checking shots against their boards: drawing the boards the picture
//! folder lacks (pictures.rs), the
//! comparison, the comparison with a before run (before.rs), the lists and
//! the components' capture files.

use crate::Checker;
use crate::before::{Against, Before};
use crate::capture::Failed;
use crate::compare::{self, Finding, Side, Slots, Warning};
use crate::component_files::{self, Listing};
use crate::declared::Declared;
use crate::design::{self, DesignSystem};
use crate::findings_file::What;
use crate::lint::{self, Problem};
use crate::lists::{self, Lists};
use crate::names::{Name, Platform, Previews};
use crate::pictures::Pictures;
use crate::render::serve::Tree;
use crate::render::{self, runtime};
use crate::settings::Settings;
use crate::shots::{ElementList, Picture, Shot};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a verification looks at and may write.
pub(crate) struct Verification<'a> {
    pub(crate) root: &'a Path,
    /// The repo's screenshot settings.
    pub(crate) settings: &'a Settings,
    /// The checker's version, which keys the board pictures it draws, and its
    /// cache folder, where the design system is built and Chrome is kept.
    pub(crate) checker: &'a Checker<'a>,
    /// The shots to check.
    pub(crate) names: &'a [Name],
    /// Declared shots this run left out because they need a window.
    pub(crate) not_taken: &'a [Name],
    /// Selected shots the capture wrote no usable shot of; each fails the
    /// run.
    pub(crate) failed: &'a [Failed],
    /// Every shot the harnesses declare.
    pub(crate) declared: &'a [Declared],
    /// Where each platform's captured files are.
    pub(crate) shots_for: &'a dyn Fn(Platform) -> PathBuf,
    /// Where the boards are rendered, in `boards/`.
    pub(crate) out: &'a Path,
    /// The run the pairs are compared with: with one, only the pairs new or
    /// worse against it fail the run; with none, every pair with a finding
    /// does.
    pub(crate) before: Option<&'a Before>,
    /// The folder of the board pictures earlier runs drew (pictures.rs):
    /// with one, the run draws only the boards it lacks and keeps each new
    /// drawing there; with none, it draws every board.
    pub(crate) pictures: Option<&'a Pictures>,
}

/// One pair that was compared.
pub(crate) struct Checked {
    pub(crate) name: Name,
    pub(crate) board: ElementList,
    pub(crate) shot: ElementList,
    /// The matched elements: (board index, shot index).
    pub(crate) pairs: Vec<(usize, usize)>,
    /// How far the shot's frame sits right and down of the board's.
    pub(crate) offset: (f64, f64),
    pub(crate) board_png: PathBuf,
    pub(crate) shot_png: PathBuf,
    pub(crate) findings: Vec<Finding>,
    pub(crate) warnings: Vec<Warning>,
    /// Where the pair stands against the before run; None when the run has
    /// none.
    pub(crate) against: Option<Against>,
}

pub(crate) struct Outcome {
    /// What the run says of the runtime copy it drew the boards with.
    pub(crate) runtime: String,
    pub(crate) checked: Vec<Checked>,
    pub(crate) problems: Vec<Problem>,
    pub(crate) lists: Lists,
    /// Declared shots the run was not given the means to take (they need a
    /// window); neither checked nor failing.
    pub(crate) not_taken: Vec<Name>,
    /// Selected shots the capture wrote no usable shot of, with why; each
    /// fails the run and has no pair.
    pub(crate) failed: Vec<Failed>,
    /// Each component's capture file and its gaps, which fail nothing.
    pub(crate) components: Listing,
}

impl Outcome {
    /// Whether the pair's findings fail the run: it has findings, and the run
    /// has no before run, or the pair is new or worse against it.
    fn gated(pair: &Checked) -> bool {
        !pair.findings.is_empty() && !matches!(pair.against, Some(Against::Held))
    }

    /// The pairs whose findings fail the run.
    pub(crate) fn failing(&self) -> impl Iterator<Item = &Checked> {
        self.checked.iter().filter(|c| Self::gated(c))
    }

    /// The pairs with findings that are listed and do not fail the run.
    pub(crate) fn listed(&self) -> impl Iterator<Item = &Checked> {
        self.checked
            .iter()
            .filter(|c| !c.findings.is_empty() && !Self::gated(c))
    }

    /// Whether nothing fails: no gated finding, no shot without a board and no
    /// shot the capture failed.
    pub(crate) fn passed(&self) -> bool {
        self.problems.is_empty() && self.failed.is_empty() && self.failing().next().is_none()
    }
}

pub(crate) fn verify(job: &Verification) -> Result<Outcome, String> {
    let root = job.root;
    let runtime = runtime::check(root).map_err(|e| e.to_string())?;
    let design = DesignSystem::of(root, job.checker.cache).map_err(|e| e.to_string())?;

    let previews = Previews::of(root)?;
    let problems = lint::problems(root, &job.settings.preview_calls, &previews, job.declared);
    let boardless = |name: &Name| problems.iter().any(|p| p.name == name.to_string());
    let names: Vec<Name> = job
        .names
        .iter()
        .filter(|n| !boardless(n))
        .cloned()
        .collect();

    let mut shots = BTreeMap::new();
    for name in &names {
        let shot =
            Shot::load(&(job.shots_for)(name.platform()), name).map_err(|e| e.to_string())?;
        shots.insert(name.clone(), shot);
    }

    let boards = job.out.join("boards");
    let tree = Tree {
        root,
        design: &design,
        runtime: runtime.copy.text,
    };
    let scales: render::Scales = shots
        .iter()
        .map(|(name, shot)| (name.clone(), shot.list.scale))
        .collect();
    let hashes = render::hashes(
        root,
        &scales,
        &previews,
        &design,
        runtime.copy.sha256,
        job.checker.version,
        job.settings,
    )?;
    let to_draw = take_pictures(job.pictures, &hashes, &names, &boards)?;
    render::boards(
        tree,
        job.settings,
        &previews,
        job.checker.cache,
        &to_draw,
        &scales,
        &boards,
    )?;
    if let Some(pictures) = job.pictures {
        for name in &to_draw {
            pictures.keep_board(&hashes[name], &name.to_string(), &boards)?;
        }
    }
    let mut checked = Vec::new();
    for name in &names {
        let shot = &shots[name];
        let list =
            ElementList::load(&boards.join(format!("{name}.json"))).map_err(|e| e.to_string())?;
        let picture_file = boards.join(format!("{name}.png"));
        let picture = std::fs::read(&picture_file)
            .map_err(|e| format!("cannot read {}: {e}", picture_file.display()))?;
        let picture =
            Picture::decode(&picture).map_err(|e| format!("{}: {e}", picture_file.display()))?;
        let result = compare::compare(
            &list.elements,
            &shot.list.elements,
            Slots::new(&list.slots, list.size),
        );
        // The board's slots are areas its picture draws a stand-in for; the
        // picture check leaves them out like the board's unshot parts.
        let board_unshot: Vec<_> = list
            .unshot
            .iter()
            .copied()
            .chain(Slots::new(&list.slots, list.size).covered())
            .collect();
        let warnings = compare::warnings(
            &Side {
                elements: &list.elements,
                picture: &picture,
                scale: list.scale,
                unshot: &board_unshot,
            },
            &Side {
                elements: &shot.list.elements,
                picture: &shot.picture,
                scale: shot.list.scale,
                unshot: &shot.list.unshot,
            },
            &result.pairs,
        );
        let whats: Vec<What> = result.findings.iter().map(What::from).collect();
        checked.push(Checked {
            name: name.clone(),
            offset: compare::geometry::frame_offset(
                &list.elements,
                &shot.list.elements,
                &result.pairs,
            ),
            pairs: result
                .pairs
                .iter()
                .map(|(b, s)| (b.index, s.index))
                .collect(),
            board: list,
            shot: shot.list.clone(),
            board_png: picture_file,
            shot_png: (job.shots_for)(name.platform()).join(name.file_name("png")),
            findings: result.findings,
            warnings,
            against: job
                .before
                .map(|before| before.against(&name.to_string(), &whats)),
        });
    }
    let ready = design::ready_boards(root).map_err(|e| e.to_string())?;
    let lists = lists::lists(&hashed_all(job.declared), &ready, &problems);
    let components = component_files::list(root, &previews, job.settings, job.declared)?;
    Ok(Outcome {
        runtime: runtime.said,
        checked,
        problems,
        lists,
        not_taken: job.not_taken.to_vec(),
        failed: job.failed.to_vec(),
        components,
    })
}

/// Copies into <boards> the picture <pictures> keeps for the hash of each
/// of <names>, and returns the names whose board it keeps none of, which the
/// run draws. With no picture folder, the run draws every board.
pub(crate) fn take_pictures(
    pictures: Option<&Pictures>,
    hashes: &BTreeMap<Name, String>,
    names: &[Name],
    boards: &Path,
) -> Result<Vec<Name>, String> {
    let Some(pictures) = pictures else {
        return Ok(names.to_vec());
    };
    let mut to_draw = Vec::new();
    for name in names {
        if !pictures.take_board(&hashes[name], &name.to_string(), boards)? {
            to_draw.push(name.clone());
        }
    }
    crate::log::note(format_args!(
        "took {} of {} boards from the picture folder; drawing {}",
        names.len() - to_draw.len(),
        names.len(),
        to_draw.len()
    ));
    Ok(to_draw)
}

fn hashed_all(declared: &[Declared]) -> Vec<Name> {
    declared.iter().map(|d| d.name.clone()).collect()
}

/// The mark a line of the pair gets for where it stands against the before
/// run.
pub(crate) fn note(against: Option<Against>) -> &'static str {
    match against {
        Some(Against::New) => "new against the base",
        Some(Against::Worse) => "worse than the base",
        Some(Against::Held) | None => "",
    }
}

/// The note as a bracketed suffix of a line, empty when there is none.
pub(crate) fn mark(against: Option<Against>) -> String {
    match note(against) {
        "" => String::new(),
        note => format!(" ({note})"),
    }
}

/// Prints what the run found: the pairs that fail it first, then the pairs
/// whose findings are only listed, then a summary naming both.
pub(crate) fn print(outcome: &Outcome) {
    for problem in &outcome.problems {
        crate::log::outcome(&problem.sentence);
    }
    crate::capture::print_failed(&outcome.failed);
    for (label, pairs) in [
        ("FAIL ", outcome.failing().collect::<Vec<_>>()),
        (
            "listed, does not fail the run: ",
            outcome.listed().collect(),
        ),
    ] {
        for pair in pairs {
            for finding in &pair.findings {
                crate::log::outcome(format_args!(
                    "{label}{}{}: {}",
                    pair.name,
                    mark(pair.against),
                    finding.sentence()
                ));
            }
        }
    }
    for pair in &outcome.checked {
        for warning in &pair.warnings {
            crate::log::outcome(format_args!(
                "{}{}: warning: {}",
                pair.name,
                mark(pair.against),
                warning.sentence()
            ));
        }
    }
    for name in &outcome.not_taken {
        crate::log::outcome(format_args!("{name}: {}", crate::capture::NEEDS_WINDOW));
    }
    for (title, list) in [
        ("Boards with no shot (to build)", &outcome.lists.to_build),
        ("Shots with no board (to delete)", &outcome.lists.to_delete),
    ] {
        if !list.is_empty() {
            crate::log::outcome(format_args!("{title}:"));
            for name in list {
                crate::log::outcome(format_args!("  {name}"));
            }
        }
    }
    summarize(outcome);
}

fn names_of<'a>(pairs: impl Iterator<Item = &'a Name>) -> String {
    pairs.map(Name::to_string).collect::<Vec<_>>().join(", ")
}

/// Says plainly which pairs fail the run and which have findings that are
/// only listed.
fn summarize(outcome: &Outcome) {
    let failing: Vec<&Name> = outcome
        .failing()
        .map(|c| &c.name)
        .chain(outcome.failed.iter().map(|f| &f.name))
        .collect();
    let listed: Vec<&Name> = outcome.listed().map(|c| &c.name).collect();
    if !failing.is_empty() {
        crate::log::note(format_args!(
            "{} pair(s) fail the run: {}",
            failing.len(),
            names_of(failing.iter().copied())
        ));
    }
    if !outcome.problems.is_empty() {
        crate::log::note(format_args!(
            "{} shot(s) with no board fail the run",
            outcome.problems.len()
        ));
    }
    if !listed.is_empty() {
        crate::log::note(format_args!(
            "{} pair(s) have findings that are only listed (the base has each of them too): {}",
            listed.len(),
            names_of(listed.iter().copied())
        ));
    }
    if outcome.components.has_gaps() {
        crate::log::note(format_args!(
            "{} `builds shots components` lists them; they fail nothing",
            outcome.components.counts()
        ));
    }
    if outcome.passed() {
        crate::log::note(format_args!("no pair fails the run"));
    }
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
