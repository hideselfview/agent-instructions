//! builds-shots: checks an app's shots against its design boards. `builds
//! shots …` runs it; it reads the repo it runs in, its design files and the
//! `shots-` lines of its `.app-build` (settings.rs), and names no app.

mod before;
mod capture;
mod compare;
mod component_files;
mod declared;
mod design;
mod findings_file;
mod hex;
mod lint;
mod lists;
mod log;
mod names;
mod pictures;
mod render;
mod report;
mod settings;
mod shots;
mod states;
mod verify;

use crate::declared::Declared;
use crate::names::{Name, Platform, Prefix};
use crate::settings::Settings;
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The version of this program: a hash of every file cargo builds it from
/// (build.rs). A run's findings.json names it, and a run compares only with a
/// run of its own version (before.rs).
pub(crate) const VERSION: &str = env!("SHOTS_SOURCE");

#[derive(Parser)]
#[command(name = "builds-shots")]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Name the canvas runtime copy that draws the boards.
    Runtime,
    /// Print this program's version, which each run's findings.json names.
    Version,
    /// Take the shots into shots/, a folder for each platform, and check them.
    /// With --before, only pairs new or worse against that run fail the run,
    /// and the rest are listed; without it, every pair with a finding fails.
    Run {
        #[command(flatten)]
        selection: Selection,
        /// The findings.json of the run to compare with, written by this
        /// version of the program, such as the run of the branch's merge base
        /// with origin/main, which builds passes.
        #[arg(long)]
        before: Option<PathBuf>,
        /// The folder of the board pictures earlier runs drew, which builds
        /// keeps: the run draws only the boards it lacks and adds each new
        /// drawing.
        #[arg(long)]
        pictures: Option<PathBuf>,
    },
    /// Take the shots into a directory.
    Capture {
        #[command(flatten)]
        selection: Selection,
        /// The directory the PNGs and element lists are written to.
        #[arg(long)]
        out: PathBuf,
    },
    /// Check shots already taken against their boards.
    Check {
        /// The directory holding every platform's PNGs and element lists.
        #[arg(long)]
        shots: PathBuf,
        /// The directory the report is written to, with the rendered boards.
        #[arg(long)]
        out: PathBuf,
        /// The commit the shots were taken at, for the report.
        #[arg(long)]
        sha: String,
        /// The folder of the board pictures earlier runs drew, as for `run`.
        #[arg(long)]
        pictures: Option<PathBuf>,
    },
    /// Fail when a declared shot has no board.
    Lint,
    /// List each design-system component's preview rows whose shot its
    /// capture file (Components/<Component>.<extension> under the platform's
    /// declare folder) does not declare, and each capture file with no
    /// preview. The listing fails nothing.
    /// List each design-system component state that a board draws and no row
    /// of the component's preview shows: a value of a state prop, or a
    /// component no preview row draws, with the boards that draw it. A prop
    /// is part of a state when it is true, or a code word (a lower-case
    /// letter, then letters, digits, - and _) that the component never draws
    /// as words; children, callbacks, numbers and words are not. With
    /// --against, list only the states that the listing at that revision
    /// does not hold, and fail when there is one.
    States {
        /// Draw the boards and previews as they are at this revision, not as
        /// the working tree holds them.
        #[arg(long)]
        at: Option<String>,
        /// List only the states that the listing at this revision does not
        /// hold, and fail when there is one.
        #[arg(long)]
        against: Option<String>,
    },
    Components {
        /// A component to show with every row, such as MacRow; repeat for
        /// several. With none, every component with a row its capture file
        /// does not declare.
        components: Vec<String>,
    },
}

#[derive(Args)]
struct Selection {
    /// A platform to take shots on (mac, android, web); repeat for several.
    /// With none, the platforms of the --select prefixes, or every platform
    /// the repo's .app-build gives a shots-capture line.
    #[arg(long = "platform", value_parser = parse_platform)]
    platforms: Vec<Platform>,
    /// A board-name prefix: `mac.welcome.` is the Mac welcome boards,
    /// `mac.component.row.` one component's states. Repeat for several. With
    /// none, every shot the platform declares.
    #[arg(long = "select", value_parser = Prefix::parse)]
    prefixes: Vec<Prefix>,
    /// Also take the shots shown in their windows, which bring the app to the
    /// front and take the focus.
    #[arg(long)]
    windows: bool,
}

fn parse_platform(text: &str) -> Result<Platform, String> {
    Platform::parse(text).ok_or_else(|| "a platform is mac, android or web".to_string())
}

impl Selection {
    fn platforms(&self, settings: &Settings) -> Vec<Platform> {
        let mut platforms = if !self.platforms.is_empty() {
            self.platforms.clone()
        } else if !self.prefixes.is_empty() {
            self.prefixes.iter().map(Prefix::platform).collect()
        } else {
            settings.platforms()
        };
        platforms.sort();
        platforms.dedup();
        platforms
    }
}

/// Takes the shots `selection` selects, each platform's into the folder
/// `out_for` gives, with the repo's capture commands. Returns the declared
/// shots the selection takes and those it leaves out because they need a
/// window the run was not given.
fn capture(
    root: &Path,
    settings: &Settings,
    declared: &[Declared],
    selection: &Selection,
    out_for: impl Fn(Platform) -> PathBuf,
) -> Result<capture::Taken, String> {
    let mut all = capture::Taken {
        names: Vec::new(),
        not_taken: Vec::new(),
    };
    for platform in selection.platforms(settings) {
        let command = settings
            .capture
            .iter()
            .find(|(p, _)| *p == platform)
            .map(|(_, command)| command.as_slice())
            .unwrap_or_default();
        let out = std::path::absolute(out_for(platform)).map_err(|e| e.to_string())?;
        let taken = capture::capture(
            root,
            command,
            declared,
            platform,
            &selection.prefixes,
            selection.windows,
            &out,
        )
        .map_err(|e| e.to_string())?;
        all.names.extend(taken.names);
        all.not_taken.extend(taken.not_taken);
    }
    Ok(all)
}

/// The commit at the checkout's head.
fn commit(root: &Path) -> Result<String, String> {
    let text = boards::git_text(
        boards::git(root).args(["rev-parse", "--short=9", "HEAD"]),
        "name the head commit",
    )
    .map_err(|e| e.to_string())?;
    Ok(text.trim().to_string())
}

/// The program's cache folder: `BUILDS_SHOTS_CACHE`, which builds sets to a
/// folder on its build volume, else `~/.cache/builds-shots`. It holds pinned
/// Chrome and the scratch folders the design system is built in.
pub(crate) fn cache_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("BUILDS_SHOTS_CACHE") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "neither BUILDS_SHOTS_CACHE nor HOME is set".to_string())?;
    Ok(PathBuf::from(home).join(".cache/builds-shots"))
}

/// The repo the program runs in: git's top of the working directory.
fn repository_root() -> Result<PathBuf, String> {
    let here =
        std::env::current_dir().map_err(|e| format!("cannot read the working directory: {e}"))?;
    let top = boards::git_text(
        boards::git(&here).args(["rev-parse", "--show-toplevel"]),
        "find the repo builds shots runs in",
    )
    .map_err(|e| e.to_string())?;
    Ok(PathBuf::from(top.trim()))
}

fn header(commit: String, base: Option<String>, outcome: &verify::Outcome) -> report::Header {
    report::Header {
        commit,
        base,
        runtime: outcome.runtime.clone(),
        chrome: format!("Chrome for Testing {}", render::chrome::PINNED),
    }
}

/// Prints the outcome, writes the report into `report_dir` and, in CI, the
/// step summary. True when nothing fails.
fn finish(
    outcome: &verify::Outcome,
    header: &report::Header,
    report_dir: &Path,
) -> Result<bool, String> {
    verify::print(outcome);
    report::write(report_dir, outcome, header)?;
    crate::log::note(format_args!(
        "report at {}",
        report_dir.join("index.html").display()
    ));
    if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&summary)
            .map_err(|e| format!("cannot open the step summary: {e}"))?;
        file.write_all(report::summary(outcome, header).as_bytes())
            .map_err(|e| format!("cannot write the step summary: {e}"))?;
    }
    Ok(outcome.passed())
}

fn run(action: Action) -> Result<bool, String> {
    let root = repository_root()?;
    let settings = Settings::load(&root).map_err(|e| e.to_string())?;
    let read_declared = || declared::read(&root, &settings.declared).map_err(|e| e.to_string());
    match action {
        Action::Run {
            selection,
            before,
            pictures,
        } => {
            let before = before
                .map(|path| {
                    let before = before::Before::load(&path)?;
                    crate::log::note(format_args!(
                        "comparing with the run of {} ({})",
                        before.commit(),
                        path.display()
                    ));
                    Ok::<_, String>(before)
                })
                .transpose()?;
            let declared = read_declared()?;
            let shots = root.join("shots");
            let taken = capture(&root, &settings, &declared, &selection, |p| {
                shots.join(p.word())
            })?;
            let cache = cache_dir()?;
            let outcome = verify::verify(&verify::Verification {
                root: &root,
                settings: &settings,
                cache: &cache,
                names: &taken.names,
                not_taken: &taken.not_taken,
                declared: &declared,
                shots_for: &|p| shots.join(p.word()),
                out: &shots,
                before: before.as_ref(),
                pictures: pictures.map(pictures::Pictures::new).as_ref(),
            })?;
            let base = before.as_ref().map(|b| b.commit().to_string());
            let header = header(commit(&root)?, base, &outcome);
            finish(&outcome, &header, &shots.join("report"))
        }
        Action::Capture { selection, out } => {
            let declared = read_declared()?;
            capture(&root, &settings, &declared, &selection, |_| out.clone())?;
            Ok(true)
        }
        Action::Check {
            shots,
            out,
            sha,
            pictures,
        } => {
            let declared = read_declared()?;
            let names: Vec<Name> = declared.iter().map(|d| d.name.clone()).collect();
            for platform in settings.platforms() {
                let wanted: Vec<Name> = names
                    .iter()
                    .filter(|n| n.platform() == platform)
                    .cloned()
                    .collect();
                capture::check_complete(&shots, platform, &wanted, &names)
                    .map_err(|e| e.to_string())?;
            }
            let cache = cache_dir()?;
            let outcome = verify::verify(&verify::Verification {
                root: &root,
                settings: &settings,
                cache: &cache,
                names: &names,
                not_taken: &[],
                declared: &declared,
                shots_for: &|_| shots.clone(),
                out: &out,
                before: None,
                pictures: pictures.map(pictures::Pictures::new).as_ref(),
            })?;
            let header = header(sha, None, &outcome);
            finish(&outcome, &header, &out)
        }
        Action::Lint => {
            let declared = read_declared()?;
            let previews = names::Previews::of(&root)?;
            let problems = lint::problems(&root, &settings.preview_calls, &previews, &declared);
            for problem in &problems {
                crate::log::note(&problem.sentence);
            }
            Ok(problems.is_empty())
        }
        Action::Components { components } => {
            let declared = read_declared()?;
            let previews = names::Previews::of(&root)?;
            let listing = component_files::list(&root, &previews, &settings, &declared)?;
            component_files::check_names(&listing, &components)?;
            for line in component_files::lines(&listing, &components) {
                crate::log::outcome(line);
            }
            Ok(true)
        }
        Action::States { at, against } => {
            let cache = cache_dir()?;
            let now = states::read(&root, at.as_deref(), &cache)?;
            let listed = match &against {
                Some(rev) => states::added(now, &states::read(&root, Some(rev), &cache)?),
                None => now,
            };
            for line in states::lines(&listed, against.as_deref()) {
                crate::log::outcome(line);
            }
            Ok(against.is_none() || listed.is_empty())
        }
        Action::Version => unreachable!("main prints the version before it reads a repo"),
        Action::Runtime => {
            let checked = render::runtime::check(&root).map_err(|e| e.to_string())?;
            crate::log::note(format_args!(
                "runtime {}: {}",
                checked.copy.sha256, checked.said
            ));
            Ok(true)
        }
    }
}

fn main() -> ExitCode {
    let outcome = match Cli::parse().action {
        Action::Version => {
            println!("{VERSION}");
            Ok(true)
        }
        action => run(action),
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            crate::log::note(message);
            ExitCode::FAILURE
        }
    }
}
