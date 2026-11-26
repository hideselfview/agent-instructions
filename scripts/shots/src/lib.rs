//! The shots checker: checks an app's shots against its design boards.
//! builds runs it as `builds shots …` (`main`), in the builds program; it
//! reads the repo it runs in, its design files and the `shots-` lines of its
//! `.app-build` (settings.rs), and names no app.

mod backend;
mod before;
mod board_sizes;
mod capture;
mod capture_marks;
mod cli;
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

pub use crate::capture_marks::{CAPTURE_MARKS_DIR, CaptureMarkError, CaptureMarks};
use crate::cli::{Action, Cli, Selection};
pub use crate::declared::Declared;
use crate::names::App;
pub use crate::names::{ComponentName, Name, NameError, Prefix, ScreenName};
pub use crate::pictures::app_picture_tree;
pub use crate::report::{board_picture, shot_picture};
use crate::settings::Settings;
use clap::Parser;
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

/// The apps and declare folders in the shots checker's settings.
pub fn declared_folders(config: &str) -> Result<Vec<(App, String)>, String> {
    Ok(Settings::parse(config).map_err(|e| e.to_string())?.declared)
}

/// Each shot the harnesses in <folders> declare under <root>, in name order.
pub fn declared_scenes(root: &Path, folders: &[(App, String)]) -> Result<Vec<Declared>, String> {
    declared::read(root, folders).map_err(|e| e.to_string())
}

/// The folder of the host's Chrome slot files, in the build volume's root. A
/// name that starts with a dot is one of the volume's own, which `builds
/// status` leaves out.
pub const CHROME_SLOTS_FOLDER: &str = ".chrome-slots";

/// What builds runs the checker with.
pub struct Checker<'a> {
    /// The checker's versions, which builds computes (builds' build.rs).
    pub versions: &'a Versions,
    /// The cache folder, which builds keeps on its build volume (it holds
    /// pinned Chrome and the scratch folders the design system is built in),
    /// and the folder of the SF Mono files the boards' pages load.
    pub folders: &'a boards::Folders,
}

impl Selection {
    fn mark_taken(
        &self,
        pictures: &pictures::Pictures,
        app: &App,
        tree: &str,
    ) -> Result<(), String> {
        pictures
            .mark_taken(
                app,
                tree,
                &taken_prefixes(app, &self.prefixes),
                self.windows,
            )
            .map_err(|error| error.to_string())
    }

    /// The apps the run takes shots of: those named with `--app`, else those
    /// the `--select` prefixes start with, else every app the settings give a
    /// capture command. Fails for a name that is no app of the settings.
    fn apps(&self, settings: &Settings) -> Result<Vec<App>, String> {
        let names: Vec<&str> = if !self.apps.is_empty() {
            self.apps.iter().map(String::as_str).collect()
        } else {
            self.prefixes.iter().map(Prefix::app).collect()
        };
        let mut apps = if names.is_empty() {
            settings.capturing_apps()
        } else {
            names
                .into_iter()
                .map(|name| {
                    settings.app(name).ok_or_else(|| {
                        format!(
                            "{name} is no app: .app-build names an app with an `app` line, and mac, android and web need none"
                        )
                    })
                })
                .collect::<Result<Vec<App>, String>>()?
        };
        apps.sort();
        apps.dedup();
        Ok(apps)
    }
}

/// Takes the shots `selection` selects, each app's into the folder
/// `out_for` gives, with the repo's capture commands. With a picture folder
/// and an app source tree (<kept>), an app whose every selected shot the
/// folder keeps for the tree takes none and gets those, and each shot an
/// app takes is kept there. Returns the declared shots the selection
/// takes and those it leaves out because they take the focus and the run was
/// not given `--windows`.
fn capture(
    root: &Path,
    checker: &Checker,
    settings: &Settings,
    declared: &[Declared],
    selection: &Selection,
    kept: Option<Kept>,
    out_for: impl Fn(&App) -> PathBuf,
) -> Result<capture::Taken, String> {
    let mut all = capture::Taken {
        names: Vec::new(),
        not_taken: Vec::new(),
        failed: Vec::new(),
    };
    for app in &selection.apps(settings)? {
        let out = std::path::absolute(out_for(app)).map_err(|e| e.to_string())?;
        if let Some(Kept {
            pictures,
            tree,
            no_capture,
            retake,
        }) = kept
        {
            let taken = capture::taken(
                declared,
                app,
                &selection.prefixes,
                selection.windows,
                selection.base,
            )
            .map_err(|e| e.to_string())?;
            capture::clear(&out, app).map_err(|e| e.to_string())?;
            if !retake && pictures.take_shots(app, tree, &taken.names, &out)? {
                crate::log::note(format_args!(
                    "took the {} {} shots of app tree {tree} from the picture folder; capturing none",
                    taken.names.len(),
                    app.name
                ));
                selection.mark_taken(pictures, app, tree)?;
                all.names.extend(taken.names);
                all.not_taken.extend(taken.not_taken);
                continue;
            }
            if no_capture {
                return Err(format!(
                    "the picture folder keeps not every selected {} shot of app tree {tree}, and the run may capture none (--no-capture)",
                    app.name
                ));
            }
        }
        let command = settings
            .capture
            .iter()
            .find(|(a, _)| a == app)
            .map(|(_, command)| command.as_slice())
            .unwrap_or_default();
        let taken = capture::capture(
            root,
            command,
            declared,
            app,
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
            pictures.keep_shots(app, tree, &taken.names, &out)?;
            // The mark says the folder keeps every shot the prefixes select,
            // which a failed shot makes untrue.
            if taken.failed.is_empty() {
                selection.mark_taken(pictures, app, tree)?;
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

/// The prefixes of <app> a run of <prefixes> takes every shot of: those of the
/// app, or the app's every shot when none is given.
fn taken_prefixes(app: &App, prefixes: &[Prefix]) -> Vec<Prefix> {
    if prefixes.is_empty() {
        return vec![Prefix::all_of(app)];
    }
    prefixes
        .iter()
        .filter(|prefix| prefix.app() == app.name)
        .cloned()
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
            empty_before,
            pictures,
            app_tree,
            no_capture,
            retake,
            capture_only,
        } => {
            // The one time the run marks each picture it takes used.
            let now = std::time::SystemTime::now();
            let pictures = pictures.map(|dir| pictures::Pictures::new(dir, now));
            let before = match (before, empty_before) {
                (Some(path), _) => {
                    let before = before::Before::load(&path, &checker.versions.comparison)?;
                    crate::log::note(format_args!(
                        "comparing with the run of {} ({})",
                        before.commit(),
                        path.display()
                    ));
                    Some(before)
                }
                (None, Some(commit)) => {
                    crate::log::note(format_args!(
                        "comparing with an empty run of {commit}: every scene is new"
                    ));
                    Some(before::Before::empty(&commit))
                }
                (None, None) => None,
            };
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
                |app| shots.join(&app.name),
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
                shots_for: &|app| shots.join(&app.name),
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
            for app in &selection.apps(&settings)? {
                let taken = capture::taken(
                    &declared,
                    app,
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
            for app in &settings.capturing_apps() {
                let wanted: Vec<Name> = names.iter().filter(|n| n.app() == app).cloned().collect();
                capture::check_complete(&shots, app, &wanted, &names).map_err(|e| e.to_string())?;
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
            let design_only = component_files::design_only_problems(&root, &previews, &settings)?;
            for problem in &design_only {
                crate::log::note(problem);
            }
            Ok(problems.is_empty() && design_only.is_empty())
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
        Action::States(args) => states::command(checker, &root, args),
        Action::Draws(args) => draws::command(checker, &root, args),
        Action::Version | Action::Recheck { .. } | Action::Chrome => {
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
        Action::Chrome => {
            let started = tokio::runtime::Runtime::new()
                .map_err(|e| e.to_string())
                .map(|runtime| runtime.block_on(render::chrome::start(checker.folders)));
            started.map(|started| {
                println!("{}", started.line());
                true
            })
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
    use crate::names::Apps;

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
        let name = Name::parse(name, &Apps::default()).unwrap();
        let list = crate::shots::tests::LIST.replace("mac.welcome.a", &name.to_string());
        crate::shots::tests::write_pair(dir, &name, &list, 800, 380);
    }

    fn declared(names: &[&str]) -> Vec<Declared> {
        names
            .iter()
            .map(|name| Declared {
                name: Name::parse(name, &Apps::default()).unwrap(),
                file: PathBuf::from("Scenes.swift"),
                marks: crate::declared::Marks::default(),
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
        let folders = boards::Folders {
            cache: dir.path().to_path_buf(),
            fonts: None,
            volume: dir.path().to_path_buf(),
        };
        let checker = Checker {
            versions: &versions,
            folders: &folders,
        };
        let shots = declared(&["mac.welcome.a", "mac.settings.b", "mac.other.c"]);
        let pictures =
            pictures::Pictures::new(dir.path().join("pictures"), std::time::SystemTime::now());
        let selection = |prefixes: &[&str]| Selection {
            apps: Vec::new(),
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
        let out = |_: &App| dir.path().join("out");
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
        assert_eq!(
            second.names,
            vec![Name::parse("mac.settings.b", &Apps::default()).unwrap()]
        );
    }

    #[test]
    fn a_run_takes_the_apps_it_names_or_its_prefixes_start_or_its_capture_lines_give() {
        let settings = Settings::parse(
            "app windows web\nshots-capture mac ./a.sh\nshots-capture windows ./b.sh\n",
        )
        .unwrap();
        let selection = |apps: &[&str], prefixes: &[&str]| Selection {
            apps: apps.iter().map(|a| a.to_string()).collect(),
            prefixes: prefixes.iter().map(|p| Prefix::parse(p).unwrap()).collect(),
            windows: false,
            base: false,
        };
        let names = |selection: &Selection| -> Result<Vec<String>, String> {
            Ok(selection
                .apps(&settings)?
                .into_iter()
                .map(|app| app.name)
                .collect())
        };
        assert_eq!(names(&selection(&[], &[])).unwrap(), ["mac", "windows"]);
        assert_eq!(
            names(&selection(&[], &["windows.main."])).unwrap(),
            ["windows"]
        );
        assert_eq!(
            names(&selection(&["windows", "mac"], &["mac.a."])).unwrap(),
            ["mac", "windows"]
        );
        let error = names(&selection(&[], &["ios.main."])).unwrap_err();
        assert!(error.contains("ios is no app"), "{error}");
    }
}
