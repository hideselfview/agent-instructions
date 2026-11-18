//! builds-shots: checks an app's shots against its design boards. `builds
//! shots …` runs it; it reads the repo it runs in, its design files and the
//! `shots-` lines of its `.app-build` (settings.rs), and names no app.

mod capture;
mod compare;
mod declared;
mod design;
mod lint;
mod lists;
mod names;
mod record;
mod render;
mod report;
mod settings;
mod shots;
mod verify;

use crate::declared::Declared;
use crate::names::{Name, Platform, Prefix};
use crate::settings::Settings;
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The branch a local run marks its pairs new or changed against.
const LOCAL_BASE: &str = "origin/main";

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
    /// Write a results record to design/shots.toml, for whoever commits it.
    /// With no --from, the record the last `run` made (shots/record.toml).
    Record {
        /// A results file, such as the record.toml of a `check`'s --out folder.
        #[arg(long)]
        from: Option<PathBuf>,
    },
    /// Take the shots into shots/, a folder for each platform, and check them.
    /// Only pairs new or changed against origin/main's record fail the run;
    /// the rest are listed.
    Run(Selection),
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
        /// The directory the report and the results record (record.toml) are
        /// written to, with the rendered boards.
        #[arg(long)]
        out: PathBuf,
        /// A git ref whose record the pairs are marked new or changed against.
        #[arg(long)]
        base: Option<String>,
        /// The commit the shots were taken at, for the report.
        #[arg(long)]
        sha: String,
    },
    /// Fail when a declared shot or an entry of the record has no board.
    Lint,
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
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err("git could not name the head commit".to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "builds shots runs in a git repo: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
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
    eprintln!(
        "builds shots: report at {}",
        report_dir.join("index.html").display()
    );
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
        Action::Run(selection) => {
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
                base: Some(LOCAL_BASE),
                results: Some(&shots.join(record::RESULTS)),
                gate: verify::Gate::NewOrChanged,
            })?;
            let header = header(commit(&root)?, Some(LOCAL_BASE.to_string()), &outcome);
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
            base,
            sha,
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
                base: base.as_deref(),
                results: Some(&out.join(record::RESULTS)),
                gate: verify::Gate::EveryPair,
            })?;
            let header = header(sha, base, &outcome);
            finish(&outcome, &header, &out)
        }
        Action::Record { from } => {
            let from = from.unwrap_or_else(|| root.join("shots").join(record::RESULTS));
            record::Record::promote(&from, &root).map_err(|e| e.to_string())?;
            eprintln!("builds shots: wrote {}", record::FILE);
            Ok(true)
        }
        Action::Lint => {
            let declared = read_declared()?;
            let record =
                record::Record::load(&root.join(record::FILE)).map_err(|e| e.to_string())?;
            let problems = lint::problems(&root, &settings.preview_calls, &declared, &record);
            for problem in &problems {
                eprintln!("builds shots: {}", problem.sentence);
            }
            Ok(problems.is_empty())
        }
        Action::Runtime => {
            let checked = render::runtime::check(&root).map_err(|e| e.to_string())?;
            println!("runtime {}: {}", checked.copy.sha256, checked.said);
            Ok(true)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse().action) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("builds shots: {message}");
            ExitCode::FAILURE
        }
    }
}
