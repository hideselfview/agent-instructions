//! `builds checks run|list|tests`: picks the lines of a repo's `.checks`
//! (checks.rs) that some changed files match and runs or prints them.
//!
//! `whole` runs the `whole` lines on the tree as it is, the way `builds land`
//! runs them on a landing's result.
//!
//! `run` is what the pre-commit hook calls, over the staged files. Every
//! picked `fix` runs first, one at a time in file order, and then every picked
//! `check` at the same time, since the checks only read the tree the fixes
//! left. Every line runs even after one fails; the run fails at the end if any
//! did. `run` keeps a program's output and shows it only when the program
//! fails. A `fix` that
//! changed a file has that file staged again (`run` refuses a staged file
//! that also has unstaged changes before any line runs).
//!
//! `run --since <rev>` runs the same lines over the files the branch changed
//! since it left <rev>, as HEAD has them, and leaves HEAD, the index and the
//! files as they were: it refuses a changed file whose working tree differs
//! from HEAD, and a `fix` that would change a file fails, and the run puts the
//! file's bytes back. Agents check a whole branch this way without moving
//! HEAD, so an app build started meanwhile builds the branch. A range run is
//! no commit, so the hook database does not record it.
//!
//! Each `run` is recorded in the project's hook database (hook_stats.rs): the
//! staged diff, every check run or skipped, and the timings. A run whose record
//! can't be written says so on stderr and ends as the checks decided.
//!
//! git runs here with the environment the hook has, so it reads and writes the
//! index the commit uses. The programs get none of git's own variables: a
//! linked worktree's `GIT_DIR` breaks SwiftPM's package clones.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::checkout::{Checkout, git_local_env_vars};
use crate::checks::{CONFIG, Kind, Line, Planned, parse, plan};
use crate::clock::Moment;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::hook_impact;
use crate::hook_stats::{self, Change, CheckRecord, Outcome, Run, StagedFile};
use crate::host::Load;
use crate::lists::args::Args;
use crate::lists::store::{Store, database_of};
use crate::places::Places;
use crate::step_failure::RunResult;
use crate::text::note;
use boards::{git, git_text};

const USAGE: &str = "usage: builds checks run [<worktree>] | builds checks run --since <rev> [<worktree>] | builds checks whole [<worktree>] | builds checks list [<worktree>] | builds checks tests --since <rev> [--run] | builds checks stats [--project <project>] [--repo <repo>] [--since <YYYY-MM-DD>] [--change <rev>]";

/// The worktree `builds checks <args>` runs the pre-commit hook's checks in
/// when <args> is `run [<worktree>]`: <worktree>, else the current
/// directory. None for any other arguments.
pub fn run_worktree(args: &[String]) -> Option<&Path> {
    match args {
        [sub] if sub == "run" => Some(Path::new(".")),
        [sub, worktree] if sub == "run" && !worktree.starts_with("--") => {
            Some(Path::new(worktree.as_str()))
        }
        _ => None,
    }
}

pub fn cmd_checks(args: &[String]) -> Result<()> {
    if let Some(worktree) = run_worktree(args) {
        return hook(&top_of(worktree)?);
    }
    match args {
        [sub, rest @ ..] if sub == "run" => {
            let args = Args::parse(rest, &["since"], &[])?;
            let Some(since) = args.value("since")? else {
                fail!("{USAGE}")
            };
            let worktree = match args.positional() {
                [] => Path::new("."),
                [worktree] => Path::new(worktree.as_str()),
                _ => fail!("{USAGE}"),
            };
            run_range(&top_of(worktree)?, since)
        }
        [sub] if sub == "whole" => whole(&top_of(Path::new("."))?),
        [sub, worktree] if sub == "whole" => whole(&top_of(Path::new(worktree))?),
        [sub, rest @ ..] if sub == "stats" => stats(rest),
        [sub] if sub == "list" => {
            print!("{}", list_staged(&top_of(Path::new("."))?)?);
            Ok(())
        }
        [sub, worktree] if sub == "list" => {
            print!("{}", list_staged(&top_of(Path::new(worktree))?)?);
            Ok(())
        }
        [sub, rest @ ..] if sub == "tests" => {
            let (mut since, mut run) = (None, false);
            let mut rest = rest.iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--since" => since = rest.next(),
                    "--run" => run = true,
                    _ => fail!("{USAGE}"),
                }
            }
            let Some(since) = since else { fail!("{USAGE}") };
            tests(&top_of(Path::new("."))?, since, run)
        }
        _ => fail!("{USAGE}"),
    }
}

/// `whole`: the checks `builds land` runs on a landing's result, run on the
/// tree at <top> as it is now.
fn whole(top: &Path) -> Result<()> {
    let failed = run_whole(top, &mut std::io::stdout())?;
    for f in &failed {
        eprintln!("--- {}\n{}", f.name, f.output.trim_end());
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Fail::Message(format!(
            "failed: {}",
            failed
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

/// `run`, recorded in the project's hook database when the repo is in a project.
fn hook(top: &Path) -> Result<()> {
    let log = match hook_database(top) {
        Ok(log) => log,
        Err(error) => {
            note(format!("this run will not be recorded: {}", error.text()));
            None
        }
    };
    run_staged(top, log.as_deref())
}

/// The hook database of the project <top>'s repo belongs to; None for a repo in no
/// project, which has nowhere to record.
fn hook_database(top: &Path) -> Result<Option<PathBuf>> {
    let places = Places::from_env()?;
    let Some(checkout) = Checkout::at_keeping_env(top)? else {
        return Ok(None);
    };
    database_of(&places, &checkout)
}

/// `builds checks stats [--project P] [--repo R] [--since DATE] [--change REV]`.
fn stats(args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["since", "change", "repo"], &[])?;
    if !args.positional().is_empty() {
        fail!("{USAGE}");
    }
    let since = args.value("since")?;
    let start = crate::text::since_start(since)?;
    let places = Places::from_env()?;
    let store = Store::resolve(&places, &args, Path::new("."))?;
    if let Some(rev) = args.value("change")? {
        if since.is_some() {
            fail!("--change splits all the runs; it does not take --since");
        }
        let impact = hook_impact::impact(
            &store.dir.join(hook_stats::DATABASE),
            rev,
            args.value("repo")?,
        )?;
        print!("{}", hook_impact::report(&store.project, impact.as_ref()));
        return Ok(());
    }
    let database = store.dir.join(hook_stats::DATABASE);
    let repos = match args.value("repo")? {
        Some(repo) => vec![repo.to_string()],
        None => hook_stats::repos(&database)?,
    };
    if repos.is_empty() {
        println!("project {}: no runs recorded", store.project);
    }
    for (at, repo) in repos.iter().enumerate() {
        let stats = hook_stats::stats(&database, start.unwrap_or(0), repo)?;
        if at > 0 {
            println!();
        }
        print!(
            "{}",
            hook_stats::report(&store.project, repo, since, stats.as_ref())
        );
    }
    Ok(())
}

fn top_of(dir: &Path) -> Result<PathBuf> {
    let text = git_text(
        git(dir).args(["rev-parse", "--show-toplevel"]),
        &format!("find the top of {}", dir.display()),
    )?;
    Ok(PathBuf::from(text.trim()))
}

fn load(top: &Path) -> Result<Vec<Line>> {
    let file = top.join(CONFIG);
    parse(&std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?)
}

fn z_list(text: &str) -> Vec<String> {
    text.split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// `git diff --cached --name-only --diff-filter=ACMR`.
fn staged(top: &Path) -> Result<Vec<String>> {
    let text = git_text(
        git(top).args([
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--diff-filter=ACMR",
        ]),
        "list the staged files",
    )?;
    Ok(z_list(&text))
}

/// Which of <files> have changes in the working tree that are not staged.
fn unstaged_among(top: &Path, files: &[String]) -> Result<Vec<String>> {
    let text = git_text(
        git(top)
            .args(["diff", "--name-only", "-z", "--"])
            .args(files),
        "list the unstaged changes",
    )?;
    Ok(z_list(&text))
}

struct Ran {
    ok: bool,
    took: Duration,
    /// What it printed, stdout then stderr; empty when it streamed.
    output: String,
}

/// Runs <planned> in <top>, keeping its output unless <stream>. A check of a
/// recorded hook run gets <hook_check>, which this function puts in
/// `cargo_run::HOOK_CHECK_VARIABLE`, so a `builds cargo` the check runs
/// records which check started it.
fn execute(top: &Path, planned: &Planned, stream: bool, hook_check: Option<&str>) -> Result<Ran> {
    let (program, args) = planned.argv.split_first().expect("a line has a program");
    let path = if program.contains('/') {
        top.join(program)
    } else {
        PathBuf::from(program)
    };
    let mut command = Command::new(path);
    command.args(args).current_dir(top).stdin(Stdio::null());
    for name in git_local_env_vars()? {
        command.env_remove(name);
    }
    if let Some(check) = hook_check {
        command.env(crate::cargo_run::HOOK_CHECK_VARIABLE, check);
    }
    let started = Moment::now();
    if stream {
        let ok = command.status().is_ok_and(|status| status.success());
        return Ok(Ran {
            ok,
            took: started.elapsed(),
            output: String::new(),
        });
    }
    Ok(match command.output() {
        Ok(output) => Ran {
            ok: output.status.success(),
            took: started.elapsed(),
            output: format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        },
        Err(error) => Ran {
            ok: false,
            took: started.elapsed(),
            output: format!("could not run {program}: {error}\n"),
        },
    })
}

fn seconds(took: Duration) -> String {
    format!("{:.1}s", took.as_secs_f64())
}

/// The staged diff, file by file.
fn staged_diff(top: &Path) -> Result<Vec<StagedFile>> {
    let status = git_text(
        git(top).args(["diff", "--cached", "--name-status", "-z", "--no-renames"]),
        "list the staged changes",
    )?;
    let numstat = git_text(
        git(top).args(["diff", "--cached", "--numstat", "-z", "--no-renames"]),
        "measure the staged diff",
    )?;
    let mut lines = std::collections::HashMap::new();
    for entry in numstat.split('\0').filter(|entry| !entry.is_empty()) {
        let mut parts = entry.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            fail!("git diff --numstat printed {entry:?}");
        };
        // A binary file has `-` for both counts.
        lines.insert(
            path.to_string(),
            added.parse::<u64>().ok().zip(removed.parse::<u64>().ok()),
        );
    }
    let mut names = status.split('\0').filter(|entry| !entry.is_empty());
    let mut files = Vec::new();
    while let Some(letter) = names.next() {
        let Some(path) = names.next() else {
            fail!("git diff --name-status printed {letter:?} with no path")
        };
        let change = match letter {
            "A" => Change::Added,
            "D" => Change::Deleted,
            _ => Change::Modified,
        };
        files.push(StagedFile {
            path: path.to_string(),
            change,
            lines: lines.get(path).copied().flatten(),
        });
    }
    Ok(files)
}

/// What a run records before any check runs.
fn begin(top: &Path) -> Result<Run> {
    let checkout = Checkout::at_keeping_env(top)?;
    let repo = checkout.as_ref().map(Checkout::repo).unwrap_or_default();
    let branch = match &checkout {
        Some(checkout) => checkout.branch()?,
        None => None,
    };
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs() as i64;
    Ok(Run {
        at,
        repo,
        worktree: top.to_string_lossy().into_owned(),
        branch,
        base: head(top),
        passed: false,
        plan_ms: 0,
        total_ms: 0,
        files: staged_diff(top)?,
        checks: Vec::new(),
    })
}

/// The commit HEAD points at; None in a repo with no commit.
fn head(top: &Path) -> Option<String> {
    let output = git(top)
        .args(["rev-parse", "--verify", "-q", "HEAD"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !text.is_empty()).then_some(text)
}

fn millis(took: Duration) -> u64 {
    took.as_millis() as u64
}

/// Runs the checks and fixes the staged files pick. With <log>, the run is
/// recorded there; a record that can't be made or written is a warning and
/// changes nothing about the result.
pub fn run_staged(top: &Path, log: Option<&Path>) -> Result<()> {
    let started = Moment::now();
    let mut run = match log {
        Some(_) => match begin(top) {
            Ok(run) => Some(run),
            Err(error) => {
                note(format!("this run will not be recorded: {}", error.text()));
                None
            }
        },
        None => None,
    };
    let result = run_checks(top, started, run.as_mut());
    if let (Some(mut run), Some(log)) = (run, log) {
        run.passed = result.is_ok();
        run.total_ms = millis(started.elapsed());
        if let Err(error) = hook_stats::record(log, &run) {
            note(format!("this run was not recorded: {}", error.text()));
        }
    }
    result
}

fn run_checks(top: &Path, started: Moment, mut run: Option<&mut Run>) -> Result<()> {
    let lines = load(top)?;
    let staged = staged(top)?;
    let partly = unstaged_among(top, &staged)?;
    if !partly.is_empty() {
        fail!(
            "refusing to run: {} staged and also changed in the working tree after staging: {}. The checks read the working tree, so they would not check what is committed; stage the rest of each file or put it aside (git add -p, git restore -p), then commit again",
            if partly.len() == 1 {
                "this file is"
            } else {
                "these files are"
            },
            partly.join(", ")
        );
    }
    let picked = plan(
        top,
        &lines,
        &[Kind::Check, Kind::Fix],
        &staged,
        false,
        Some("HEAD"),
    )?;
    if let Some(run) = run.as_deref_mut() {
        run.plan_ms = millis(started.elapsed());
        run.checks = lines
            .iter()
            .filter(|line| matches!(line.kind, Kind::Check | Kind::Fix))
            .map(|line| CheckRecord {
                name: line.name.clone(),
                kind: line.kind,
                outcome: Outcome::Skipped,
            })
            .collect();
    }
    if picked.is_empty() {
        return Ok(());
    }
    // For each picked line, build the value `builds cargo` records: the run's
    // start time and the line's position among the recorded checks.
    let hook_checks: Vec<Option<String>> = picked
        .iter()
        .map(|planned| {
            run.as_deref().map(|run| {
                let position = run
                    .checks
                    .iter()
                    .position(|c| c.name == planned.line.name)
                    .expect("every picked line is recorded");
                crate::cargo_run::hook_check_value(run.at, position as u32)
            })
        })
        .collect();
    run_picked(top, &picked, &hook_checks, Fixes::Stage, run, started)
}

/// What a run does with the files a `fix` changed.
#[derive(Clone, Copy)]
enum Fixes {
    /// The hook stages them again, and the commit takes them.
    Stage,
    /// A range run puts each file's bytes back and fails the fix, since the
    /// branch's commits hold the files as they were.
    PutBack,
}

/// Runs <picked> in <top>, each line with its value of <hook_checks>: the
/// fixes first, one at a time in file order, doing with the files they change
/// what <fixes> says, then every check at the same time. Records each line in
/// <run> and fails, naming the lines that failed, when any did.
fn run_picked(
    top: &Path,
    picked: &[Planned],
    hook_checks: &[Option<String>],
    fixes: Fixes,
    mut run: Option<&mut Run>,
    started: Moment,
) -> Result<()> {
    let mut failed: Vec<&str> = Vec::new();
    // Run the fixes first, one at a time in file order. A fix may rewrite
    // files, and the checks then read what it left.
    for (planned, hook_check) in picked.iter().zip(hook_checks) {
        if planned.line.kind != Kind::Fix {
            continue;
        }
        let mut before: Vec<(String, Option<Vec<u8>>)> = Vec::new();
        for file in &planned.files {
            before.push((file.clone(), contents(top, file)?));
        }
        let load = Load::here();
        let mut ran = execute(top, planned, false, hook_check.as_deref())?;
        let mut changed: Vec<&(String, Option<Vec<u8>>)> = Vec::new();
        for entry in &before {
            if contents(top, &entry.0)? != entry.1 {
                changed.push(entry);
            }
        }
        match fixes {
            Fixes::Stage => {
                let names: Vec<&str> = changed.iter().map(|(file, _)| file.as_str()).collect();
                if ran.ok && !names.is_empty() {
                    git_text(
                        git(top).args(["add", "--"]).args(&names),
                        "stage the files a fix changed",
                    )?;
                }
            }
            Fixes::PutBack => {
                for (file, was) in &changed {
                    let path = top.join(file);
                    match was {
                        Some(bytes) => std::fs::write(&path, bytes)
                            .context(|| format!("putting back {}", path.display()))?,
                        None => {
                            if path.exists() {
                                std::fs::remove_file(&path)
                                    .context(|| format!("removing {}", path.display()))?;
                            }
                        }
                    }
                }
                if !changed.is_empty() {
                    let names: Vec<&str> = changed.iter().map(|(file, _)| file.as_str()).collect();
                    ran.ok = false;
                    ran.output = format!(
                        "{} would change {}, so the branch's commits do not pass it; builds put the files back\n{}",
                        planned.line.name,
                        names.join(", "),
                        ran.output
                    );
                }
            }
        }
        finish_line(planned, load, ran, run.as_deref_mut(), &mut failed);
    }
    // Then run every check at the same time, since the checks only read the
    // tree. Each check measures its own time, and the results print in file
    // order.
    let checks: Vec<(&Planned, &Option<String>)> = picked
        .iter()
        .zip(hook_checks)
        .filter(|(planned, _)| planned.line.kind == Kind::Check)
        .collect();
    let ran_all: Vec<(Load, Result<Ran>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = checks
            .iter()
            .map(|(planned, hook_check)| {
                scope.spawn(move || {
                    let load = Load::here();
                    (load, execute(top, planned, false, hook_check.as_deref()))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a check's thread does not panic"))
            .collect()
    });
    for ((planned, _), (load, ran)) in checks.iter().zip(ran_all) {
        finish_line(planned, load, ran?, run.as_deref_mut(), &mut failed);
    }
    note(format!(
        "{} run, {} failed, {}",
        picked.len(),
        failed.len(),
        seconds(started.elapsed())
    ));
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Fail::Message(format!("failed: {}", failed.join(", "))))
    }
}

/// The bytes of <file> under <top>, or None when there is no such file.
/// Fails, naming the file, when it cannot be read.
fn contents(top: &Path, file: &str) -> Result<Option<Vec<u8>>> {
    let path = top.join(file);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => fail!("reading {}: {error}", path.display()),
    }
}

/// `run --since <since>`: runs the check and fix lines that the files the
/// branch at <top> changed since it left <since> match, over those files as
/// HEAD has them, and leaves HEAD, the index and the files as they were.
/// Refuses, before any line runs, a <since> that is no commit and a changed
/// file whose working tree differs from HEAD. A fix that would change a file
/// fails, and the run puts the file back (`Fixes::PutBack`).
pub fn run_range(top: &Path, since: &str) -> Result<()> {
    let started = Moment::now();
    let lines = load(top)?;
    let output = git(top)
        .args(["merge-base", since, "HEAD"])
        .stdin(Stdio::null())
        .output()
        .context(|| "running git merge-base".to_string())?;
    if !output.status.success() {
        fail!(
            "{since} is no commit that HEAD shares history with: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let files = z_list(&git_text(
        git(top).args([
            "diff",
            "--name-only",
            "-z",
            "--diff-filter=ACMR",
            &base,
            "HEAD",
        ]),
        "list the files the branch changed",
    )?);
    if files.is_empty() {
        note(format!("the branch changed no file since {since}"));
        return Ok(());
    }
    let edited = z_list(&git_text(
        git(top)
            .args(["diff", "--name-only", "-z", "HEAD", "--"])
            .args(&files),
        "list the changes since HEAD",
    )?);
    if !edited.is_empty() {
        fail!(
            "refusing to run: the branch changed {} and the working tree differs from HEAD there: {}. The checks read the working tree, so they would not check the branch's commits; commit the changes or put them aside (git restore -p), then run again",
            if edited.len() == 1 {
                "this file"
            } else {
                "these files"
            },
            edited.join(", ")
        );
    }
    let picked = plan(
        top,
        &lines,
        &[Kind::Check, Kind::Fix],
        &files,
        false,
        Some(&base),
    )?;
    let hook_checks = vec![None; picked.len()];
    run_picked(top, &picked, &hook_checks, Fixes::PutBack, None, started)
}

/// Prints how <planned> ran, prints its output and adds its name to <failed>
/// when it failed, and records it in <run>, with this Mac's <load> when it
/// started.
fn finish_line<'a>(
    planned: &'a Planned,
    load: Load,
    ran: Ran,
    run: Option<&mut Run>,
    failed: &mut Vec<&'a str>,
) {
    let name = planned.line.name.as_str();
    if let Some(run) = run {
        let record = run
            .checks
            .iter_mut()
            .find(|c| c.name == name)
            .expect("every picked line is recorded");
        record.outcome = Outcome::Ran {
            result: if ran.ok {
                RunResult::Passed
            } else {
                RunResult::Failed(Some(crate::hook_check_failure::said(
                    &ran.output,
                    &planned.files,
                )))
            },
            handed: (
                if planned.line.command.iter().any(|arg| arg == "{files}") {
                    planned.files.len() as u32
                } else {
                    0
                },
                if planned.line.command.iter().any(|arg| arg == "{crates}") {
                    planned.argv.iter().filter(|arg| *arg == "-p").count() as u32
                } else {
                    0
                },
            ),
            ms: Some(millis(ran.took)),
            started: Some(load),
            cargo: None,
        };
    }
    let kind = planned.line.kind.word();
    if ran.ok {
        note(format!("{kind} {name}: ok, {}", seconds(ran.took)));
    } else {
        note(format!("{kind} {name}: FAILED, {}", seconds(ran.took)));
        eprint!("{}", ran.output);
        failed.push(name);
    }
}

/// A `whole` check that failed in `run_whole`, with what it printed.
pub struct WholeFailure {
    pub name: String,
    pub output: String,
}

/// Runs every `whole` line of the repo's `.checks` on the tree at <top>,
/// whatever changed, all at the same time, printing each one's name, result and
/// time to <out>. Every line runs even after one fails. A repo with no `.checks` has none to run.
pub fn run_whole(top: &Path, out: &mut dyn std::io::Write) -> Result<Vec<WholeFailure>> {
    if !top.join(CONFIG).is_file() {
        return Ok(Vec::new());
    }
    let started = Moment::now();
    let lines = load(top)?;
    let picked: Vec<Planned> = plan(top, &lines, &[Kind::Check], &[], true, None)?
        .into_iter()
        .filter(|planned| planned.line.whole)
        .collect();
    // The checks only read the tree, so they run at the same time; each one's
    // time is its own, and the results print in file order.
    let ran_all: Vec<Result<Ran>> = std::thread::scope(|scope| {
        let handles: Vec<_> = picked
            .iter()
            .map(|planned| scope.spawn(move || execute(top, planned, false, None)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a check's thread does not panic"))
            .collect()
    });
    let mut failed = Vec::new();
    for (planned, ran) in picked.iter().zip(ran_all) {
        let ran = ran?;
        let _ = writeln!(
            out,
            "whole {}: {}, {}",
            planned.line.name,
            if ran.ok { "ok" } else { "FAILED" },
            seconds(ran.took)
        );
        if !ran.ok {
            failed.push(WholeFailure {
                name: planned.line.name.clone(),
                output: ran.output,
            });
        }
    }
    let _ = writeln!(
        out,
        "{} whole checks run, {} failed, {}",
        picked.len(),
        failed.len(),
        seconds(started.elapsed())
    );
    Ok(failed)
}

pub fn list_staged(top: &Path) -> Result<String> {
    let lines = load(top)?;
    let picked = plan(
        top,
        &lines,
        &[Kind::Check, Kind::Fix],
        &staged(top)?,
        false,
        Some("HEAD"),
    )?;
    Ok(picked
        .iter()
        .map(|p| {
            format!(
                "{} {}: {}\n",
                p.line.kind.word(),
                p.line.name,
                p.argv.join(" ")
            )
        })
        .collect())
}

/// The `test` lines that the files changed in `<since>..HEAD` match, and, when
/// <since> is not a commit here, every test with the reason why.
pub fn select_tests(
    top: &Path,
    lines: &[Line],
    since: &str,
) -> Result<(Vec<Planned>, Option<String>)> {
    let known = git(top)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{since}^{{commit}}"),
        ])
        .stdin(Stdio::null())
        .output()
        .context(|| "running git rev-parse".to_string())?
        .status
        .success();
    if known {
        let text = git_text(
            git(top).args([
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                &format!("{since}..HEAD"),
            ]),
            "list the changed files",
        )?;
        Ok((
            plan(
                top,
                lines,
                &[Kind::Test],
                &z_list(&text),
                false,
                Some(since),
            )?,
            None,
        ))
    } else {
        let everything = z_list(&git_text(
            git(top).args(["ls-files", "-z"]),
            "list the files",
        )?);
        let why = format!("{since} is not in the history, so every test runs");
        Ok((
            plan(top, lines, &[Kind::Test], &everything, true, None)?,
            Some(why),
        ))
    }
}

fn tests(top: &Path, since: &str, run: bool) -> Result<()> {
    let (picked, why) = select_tests(top, &load(top)?, since)?;
    if let Some(why) = why {
        note(why);
    }
    if !run {
        for p in &picked {
            println!("{}: {}", p.line.name, p.argv.join(" "));
        }
        return Ok(());
    }
    let mut failed: Vec<&str> = Vec::new();
    for p in &picked {
        note(format!("test {}: {}", p.line.name, p.argv.join(" ")));
        let ran = execute(top, p, true, None)?;
        note(format!(
            "test {}: {}, {}",
            p.line.name,
            if ran.ok { "ok" } else { "FAILED" },
            seconds(ran.took)
        ));
        if !ran.ok {
            failed.push(p.line.name.as_str());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Fail::Message(format!("failed: {}", failed.join(", "))))
    }
}

#[cfg(test)]
#[path = "checks_run_tests.rs"]
mod tests;
