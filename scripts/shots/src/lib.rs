//! The shots checker: checks an app's shots against its design boards.
//! builds runs it as `builds shots …` (`main`), in the builds program; it
//! reads the repo it runs in, its design files and the `shots-` lines of its
//! `.app-build` (settings.rs), and names no app.

mod before;
mod board_sizes;
mod capture;
mod compare;
mod component_files;
mod declared;
mod design;
mod draws;
mod findings_file;
mod hex;
mod lint;
mod lists;
mod log;
mod names;
mod pictures;
mod recheck;
mod render;
mod report;
mod settings;
mod shots;
mod states;
mod verify;

pub use crate::declared::Declared;
pub use crate::names::{ComponentName, Name, ScreenName};
use crate::names::{Platform, Prefix};
use crate::settings::Settings;
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};

/// The two versions of the checker. The capture version is a hash of the files
/// that take a shot and draw a board: it keys the board pictures (design.rs),
/// and a run's findings.json names it, so that a run compared again
/// (recheck.rs) keeps naming the version that made its pictures. The
/// comparison version is a hash of the files that compare a shot with its
/// board: a run's findings.json names it, and a run compares only with a run
/// of its own comparison version (before.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Versions {
    pub capture: String,
    pub comparison: String,
}

/// Each shot the harnesses declare under <root>, a repo's files as they are
/// at one commit, in name order. <config> is the text of the repo's
/// `.app-build` at that commit, which names the declare folders.
pub fn declared_scenes(root: &Path, config: &str) -> Result<Vec<Declared>, String> {
    let settings = Settings::parse(config).map_err(|e| e.to_string())?;
    declared::read(root, &settings.declared).map_err(|e| e.to_string())
}

/// What builds runs the checker with.
pub struct Checker<'a> {
    /// The checker's versions, which builds computes (builds' build.rs).
    pub versions: &'a Versions,
    /// The cache folder, which builds keeps on its build volume. It holds
    /// pinned Chrome and the scratch folders the design system is built in.
    pub cache: &'a Path,
}

#[derive(Parser)]
#[command(name = "builds shots")]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Name the canvas runtime copy that draws the boards.
    Runtime,
    /// Print the checker's capture version and comparison version, which each
    /// run's findings.json names.
    Version,
    /// Take the shots into shots/, a folder for each platform, and check them.
    /// With --before, only pairs new or worse against that run fail the run,
    /// and the rest are listed; without it, every pair with a finding fails.
    Run {
        #[command(flatten)]
        selection: Selection,
        /// The findings.json of the run to compare with, written by this
        /// version of the checker, such as the run of the branch's merge base
        /// with origin/main, which builds passes.
        #[arg(long, conflicts_with = "base")]
        before: Option<PathBuf>,
        /// The folder of the board pictures earlier runs drew, which builds
        /// keeps: the run draws only the boards it lacks and adds each new
        /// drawing.
        #[arg(long)]
        pictures: Option<PathBuf>,
        /// The app source tree the shots are taken of, which builds names:
        /// when the picture folder keeps every selected shot of a platform
        /// for it, the run takes none and uses those, and otherwise it keeps
        /// the shots it takes there.
        #[arg(long, requires = "pictures")]
        app_tree: Option<String>,
        /// Fail instead of capturing when the picture folder lacks a selected
        /// shot of the app tree: builds passes it when it built no app.
        #[arg(long, requires = "app_tree")]
        no_capture: bool,
        /// Take every selected shot again, even when the picture folder keeps
        /// it for the app tree, and replace the kept pictures with the new
        /// ones. A shot taken before its window had loaded stays wrong in the
        /// folder until a run takes it again.
        #[arg(long, requires = "app_tree", conflicts_with = "no_capture")]
        retake: bool,
        /// Take every selected shot into the picture folder, mark each
        /// selected prefix taken, and end without drawing boards or comparing
        /// anything: builds takes the shots of all a lane's prefixes in one
        /// run, then runs each prefix with --no-capture.
        #[arg(
            long,
            requires = "app_tree",
            conflicts_with_all = ["before", "no_capture", "base"]
        )]
        capture_only: bool,
    },
    /// Print the number of shots a run of the same selection takes, which
    /// builds counts to give the run a time limit that grows with its work.
    Scenes {
        #[command(flatten)]
        selection: Selection,
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
    },
    /// Compare again the pairs a report holds, with this checker, and print
    /// the findings as records (the report's findings.json). The report is
    /// a cached run's folder, which holds each pair's pictures and element
    /// lists. The command draws and captures nothing and reads no repo, so a
    /// change to the comparison shows what it clears and adds on a run that
    /// another checker made.
    Recheck {
        /// The report's folder, with its findings.json and its `img` folder.
        #[arg(long)]
        report: PathBuf,
    },
    /// Fail when a declared shot has no board.
    Lint,
    /// List each design-system component's preview rows whose shot its
    /// capture file (Components/<Component>.<extension> under the platform's
    /// declare folder) does not declare, and each capture file with no
    /// preview. A component that `shots-design-only` names for a platform is
    /// left out there and counted. The listing fails nothing.
    /// List each design-system component state that a board draws and no row
    /// of the component's preview shows: a value of a state prop, or a
    /// component no preview row draws, with the boards that draw it. A prop
    /// is part of a state when it is true, or a code word (a lower-case
    /// letter, then letters, digits, - and _) that the component never draws
    /// as words; children, callbacks, numbers, words, className and icon
    /// names (icon, name, or a prop that ends in Icon) are not. With
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
    /// Print `alike <what>` or `differs <what>` for each board and component
    /// preview named, by whether it draws the same at the two revisions
    /// (picture and element boxes, not file bytes). It fails only when it
    /// cannot draw one.
    Draws {
        /// The first revision.
        #[arg(long)]
        at: String,
        /// The revision to compare with.
        #[arg(long)]
        against: String,
        /// A board's name (`design/mocks/<name>.dc.html`); repeat for several.
        #[arg(long = "board")]
        boards: Vec<String>,
        /// A component's name, whose preview page is drawn; repeat for several.
        #[arg(long = "component")]
        components: Vec<String>,
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
    /// Also take the shots that take the keyboard focus, which bring the app to
    /// the front.
    #[arg(long)]
    windows: bool,
    /// This is the run of the branch's merge base: a --select prefix that
    /// starts no declared shot selects none, because the branch adds those
    /// shots, and the run has no pairs for them. Without it, such a prefix
    /// fails the run.
    #[arg(long)]
    base: bool,
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
/// `out_for` gives, with the repo's capture commands. With a picture folder
/// and an app source tree (<kept>), a platform whose every selected shot the
/// folder keeps for the tree takes none and gets those, and each shot a
/// platform takes is kept there. Returns the declared shots the selection
/// takes and those it leaves out because they take the focus and the run was
/// not given `--windows`.
fn capture(
    root: &Path,
    checker: &Checker,
    settings: &Settings,
    declared: &[Declared],
    selection: &Selection,
    kept: Option<Kept>,
    out_for: impl Fn(Platform) -> PathBuf,
) -> Result<capture::Taken, String> {
    let mut all = capture::Taken {
        names: Vec::new(),
        not_taken: Vec::new(),
        failed: Vec::new(),
    };
    for platform in selection.platforms(settings) {
        let out = std::path::absolute(out_for(platform)).map_err(|e| e.to_string())?;
        if let Some(Kept {
            pictures,
            tree,
            no_capture,
            retake,
        }) = kept
        {
            let taken = capture::taken(
                declared,
                platform,
                &selection.prefixes,
                selection.windows,
                selection.base,
            )
            .map_err(|e| e.to_string())?;
            capture::clear(&out, platform).map_err(|e| e.to_string())?;
            if !retake && pictures.take_shots(platform, tree, &taken.names, &out)? {
                crate::log::note(format_args!(
                    "took the {} {} shots of app tree {tree} from the picture folder; capturing none",
                    taken.names.len(),
                    platform.word()
                ));
                pictures.mark_taken(
                    platform,
                    tree,
                    &taken_prefixes(platform, &selection.prefixes),
                    selection.windows,
                )?;
                all.names.extend(taken.names);
                all.not_taken.extend(taken.not_taken);
                continue;
            }
            if no_capture {
                return Err(format!(
                    "the picture folder keeps not every selected {} shot of app tree {tree}, and the run may capture none (--no-capture)",
                    platform.word()
                ));
            }
        }
        let command = settings
            .capture
            .iter()
            .find(|(p, _)| *p == platform)
            .map(|(_, command)| command.as_slice())
            .unwrap_or_default();
        let taken = capture::capture(
            root,
            command,
            declared,
            platform,
            &selection.prefixes,
            selection.windows,
            selection.base,
            &capture::Target {
                out: &out,
                measure: &|names| board_sizes::measure(root, checker, settings, names),
            },
        )
        .map_err(|e| e.to_string())?;
        if let Some(Kept { pictures, tree, .. }) = kept {
            pictures.keep_shots(platform, tree, &taken.names, &out)?;
            // The mark says the folder keeps every shot the prefixes select,
            // which a failed shot makes untrue.
            if taken.failed.is_empty() {
                pictures.mark_taken(
                    platform,
                    tree,
                    &taken_prefixes(platform, &selection.prefixes),
                    selection.windows,
                )?;
            }
        }
        all.names.extend(taken.names);
        all.not_taken.extend(taken.not_taken);
        all.failed.extend(taken.failed);
    }
    Ok(all)
}

/// What a run with `--capture-only` ends with, once it took the shots: it
/// prints each shot the harness wrote no usable files of, and passes when
/// there is none. The run marks a prefix taken only when none failed
/// (`capture`), so a later run with `--no-capture` fails on a prefix whose
/// shots are missing.
fn capture_only_outcome(taken: &capture::Taken) -> bool {
    capture::print_failed(&taken.failed);
    taken.failed.is_empty()
}

/// A picture folder and the app source tree a run keeps its shots by.
#[derive(Clone, Copy)]
struct Kept<'a> {
    pictures: &'a pictures::Pictures,
    tree: &'a str,
    /// Fail instead of capturing when the folder lacks a selected shot.
    no_capture: bool,
    /// Capture every selected shot, whether or not the folder keeps it, and
    /// replace the kept pictures.
    retake: bool,
}

/// The prefixes of <platform> a run of <prefixes> takes every shot of: those
/// of the platform, or the platform's every shot when none is given.
fn taken_prefixes(platform: Platform, prefixes: &[Prefix]) -> Vec<String> {
    if prefixes.is_empty() {
        return vec![format!("{}.", platform.word())];
    }
    prefixes
        .iter()
        .filter(|prefix| prefix.platform() == platform)
        .map(|prefix| prefix.as_str().to_string())
        .collect()
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

fn header(
    checker: &Checker,
    commit: String,
    base: Option<String>,
    outcome: &verify::Outcome,
) -> report::Header {
    report::Header {
        capture_version: checker.versions.capture.clone(),
        version: checker.versions.comparison.clone(),
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

fn run(checker: &Checker, action: Action) -> Result<bool, String> {
    let root = repository_root()?;
    let settings = Settings::load(&root).map_err(|e| e.to_string())?;
    let read_declared = || declared::read(&root, &settings.declared).map_err(|e| e.to_string());
    match action {
        Action::Run {
            selection,
            before,
            pictures,
            app_tree,
            no_capture,
            retake,
            capture_only,
        } => {
            // The one time the run marks each picture it takes used.
            let now = std::time::SystemTime::now();
            let pictures = pictures.map(|dir| pictures::Pictures::new(dir, now));
            let before = before
                .map(|path| {
                    let before = before::Before::load(&path, &checker.versions.comparison)?;
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
            let kept = pictures
                .as_ref()
                .zip(app_tree.as_deref())
                .map(|(pictures, tree)| Kept {
                    pictures,
                    tree,
                    no_capture,
                    retake,
                });
            let taken = capture(
                &root,
                checker,
                &settings,
                &declared,
                &selection,
                kept,
                |p| shots.join(p.word()),
            )?;
            if capture_only {
                return Ok(capture_only_outcome(&taken));
            }
            let outcome = verify::verify(&verify::Verification {
                root: &root,
                settings: &settings,
                checker,
                names: &taken.names,
                not_taken: &taken.not_taken,
                failed: &taken.failed,
                declared: &declared,
                shots_for: &|p| shots.join(p.word()),
                out: &shots,
                comparison: match (&before, selection.base) {
                    (Some(_), true) => unreachable!("clap refuses --base with --before"),
                    (Some(before), false) => verify::Comparison::Before(before),
                    (None, true) => verify::Comparison::Base,
                    (None, false) => verify::Comparison::Nothing,
                },
                pictures: pictures.as_ref(),
            })?;
            let base = before.as_ref().map(|b| b.commit().to_string());
            let header = header(checker, commit(&root)?, base, &outcome);
            finish(&outcome, &header, &shots.join("report"))
        }
        Action::Scenes { selection } => {
            let declared = read_declared()?;
            let mut count = 0;
            for platform in selection.platforms(&settings) {
                let taken = capture::taken(
                    &declared,
                    platform,
                    &selection.prefixes,
                    selection.windows,
                    selection.base,
                )
                .map_err(|e| e.to_string())?;
                count += taken.names.len();
            }
            println!("{count}");
            Ok(true)
        }
        Action::Capture { selection, out } => {
            let declared = read_declared()?;
            let taken = capture(
                &root,
                checker,
                &settings,
                &declared,
                &selection,
                None,
                |_| out.clone(),
            )?;
            capture::print_failed(&taken.failed);
            Ok(taken.failed.is_empty())
        }
        Action::Check { shots, out, sha } => {
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
            let outcome = verify::verify(&verify::Verification {
                root: &root,
                settings: &settings,
                checker,
                names: &names,
                not_taken: &[],
                failed: &[],
                declared: &declared,
                shots_for: &|_| shots.clone(),
                out: &out,
                comparison: verify::Comparison::Nothing,
                pictures: None,
            })?;
            let header = header(checker, sha, None, &outcome);
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
            let now = states::read(&root, at.as_deref(), checker.cache)?;
            let listed = match &against {
                Some(rev) => states::added(now, &states::read(&root, Some(rev), checker.cache)?),
                None => now,
            };
            for line in states::lines(&listed, against.as_deref()) {
                crate::log::outcome(line);
            }
            Ok(against.is_none() || listed.is_empty())
        }
        Action::Draws {
            at,
            against,
            boards,
            components,
        } => {
            let lines = draws::lines(&root, &at, &against, &boards, &components, checker.cache)?;
            for line in lines {
                println!("{line}");
            }
            Ok(true)
        }
        Action::Version | Action::Recheck { .. } => {
            unreachable!("`main` runs these before it reads a repo")
        }
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

/// Runs `builds shots <args>`: parses <args>, the words after `builds
/// shots`, and runs the command they name. Returns whether the command
/// passed, after it printed why not. Prints clap's usage and exits the
/// process when <args> name no command.
pub fn main(checker: &Checker, args: &[String]) -> bool {
    let cli =
        Cli::parse_from(std::iter::once("builds shots").chain(args.iter().map(String::as_str)));
    let outcome = match cli.action {
        Action::Version => {
            println!("capture {}", checker.versions.capture);
            println!("comparison {}", checker.versions.comparison);
            Ok(true)
        }
        Action::Recheck { report } => {
            recheck::text(&report, &checker.versions.comparison).map(|text| {
                println!("{text}");
                true
            })
        }
        action => run(checker, action),
    };
    outcome.unwrap_or_else(|message| {
        crate::log::note(message);
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("builds shots").chain(args.iter().copied()))
    }

    #[test]
    fn a_run_names_the_merge_base_s_own_run_or_a_before_run_never_both() {
        assert!(parse(&["run", "--base"]).is_ok());
        assert!(parse(&["run", "--before", "findings.json"]).is_ok());
        let error = parse(&["run", "--base", "--before", "findings.json"])
            .err()
            .expect("clap refuses the two flags together");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn a_capture_only_run_needs_the_picture_folder_and_refuses_a_comparison_or_a_failing_capture() {
        let ok = parse(&[
            "run",
            "--capture-only",
            "--pictures",
            "pictures",
            "--app-tree",
            "tree",
            "--select",
            "mac.a.",
            "--select",
            "mac.b.",
        ]);
        assert!(ok.is_ok());
        for refused in [
            &["--before", "findings.json"][..],
            &["--no-capture"][..],
            &["--base"][..],
        ] {
            let mut args = vec![
                "run",
                "--capture-only",
                "--pictures",
                "pictures",
                "--app-tree",
                "tree",
            ];
            args.extend_from_slice(refused);
            let error = parse(&args)
                .err()
                .unwrap_or_else(|| panic!("clap refuses --capture-only with {refused:?}"));
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
        let error = parse(&["run", "--capture-only"])
            .err()
            .expect("without an app tree the shots have nowhere to be kept");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    /// A shot's element list and picture, as a harness writes them.
    fn write_shot(dir: &Path, name: &str) {
        let name = Name::parse(name).unwrap();
        let list = crate::shots::tests::LIST.replace("mac.welcome.a", &name.to_string());
        crate::shots::tests::write_pair(dir, &name, &list, 800, 380);
    }

    fn declared(names: &[&str]) -> Vec<Declared> {
        names
            .iter()
            .map(|name| Declared {
                name: Name::parse(name).unwrap(),
                file: PathBuf::from("Scenes.swift"),
                in_window: false,
                takes_focus: false,
            })
            .collect()
    }

    #[test]
    fn one_capture_of_several_prefixes_fills_the_picture_folder_so_each_prefix_runs_without_capturing()
     {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        std::fs::create_dir_all(&ready).unwrap();
        write_shot(&ready, "mac.welcome.a");
        write_shot(&ready, "mac.settings.b");
        let script = dir.path().join("copy.sh");
        std::fs::write(
            &script,
            format!("cp {}/* \"$BUILDS_SHOTS_OUT\"\n", ready.display()),
        )
        .unwrap();
        let settings =
            Settings::parse(&format!("shots-capture mac sh {}\n", script.display())).unwrap();
        let versions = Versions {
            capture: "c".to_string(),
            comparison: "k".to_string(),
        };
        let checker = Checker {
            versions: &versions,
            cache: dir.path(),
        };
        let shots = declared(&["mac.welcome.a", "mac.settings.b", "mac.other.c"]);
        let pictures =
            pictures::Pictures::new(dir.path().join("pictures"), std::time::SystemTime::now());
        let selection = |prefixes: &[&str]| Selection {
            platforms: Vec::new(),
            prefixes: prefixes.iter().map(|p| Prefix::parse(p).unwrap()).collect(),
            windows: false,
            base: false,
        };
        let kept = |no_capture| {
            Some(Kept {
                pictures: &pictures,
                tree: "tree",
                no_capture,
                retake: false,
            })
        };
        let out = |_| dir.path().join("out");
        let first = capture(
            dir.path(),
            &checker,
            &settings,
            &shots,
            &selection(&["mac.welcome.", "mac.settings."]),
            kept(false),
            out,
        )
        .unwrap();
        assert!(capture_only_outcome(&first));
        let marks = |prefix: &str| {
            dir.path()
                .join(format!("pictures/mac/tree/.taken/{prefix}@no-windows"))
        };
        assert!(marks("mac.welcome.").is_file());
        assert!(marks("mac.settings.").is_file());
        // A run of one prefix that may capture none takes its shots from the folder.
        std::fs::remove_dir_all(&ready).unwrap();
        let second = capture(
            dir.path(),
            &checker,
            &settings,
            &shots,
            &selection(&["mac.settings."]),
            kept(true),
            out,
        )
        .unwrap();
        assert_eq!(second.names, vec![Name::parse("mac.settings.b").unwrap()]);
    }
}
