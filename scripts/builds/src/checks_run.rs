//! `builds checks run|list|tests`: picks the lines of a repo's `.checks`
//! (checks.rs) that some changed files match and runs or prints them.
//!
//! `run` is what the pre-commit hook calls, over the staged files. Every
//! picked `check` and `fix` runs, in file order, even after one fails; the run
//! fails at the end if any did. A program's output is kept and shown only when
//! it fails. A `fix` that changed a file has that file staged again, unless the
//! file also had unstaged changes before the fix ran: staging it would sweep
//! those into the commit, so nothing of that fix is staged and the run fails.
//!
//! Each `run` is recorded in the project's hook database (hook_stats.rs): the
//! staged diff, every check run or skipped, and the timings. A run whose record
//! can't be written says so on stderr and ends as the checks decided.
//!
//! git runs here with the environment the hook has, so it reads and writes the
//! index the commit uses. The programs get none of git's own variables: a
//! linked worktree's `GIT_DIR` breaks SwiftPM's package clones.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::checkout::{Checkout, git, git_local_env_vars, git_text};
use crate::checks::{CONFIG, Kind, Line, Planned, parse, plan};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::hook_impact;
use crate::hook_stats::{self, CheckRecord, Change, Outcome, Run, StagedFile};
use crate::lists::args::Args;
use crate::lists::store::{Store, project_of};
use crate::places::Places;
use crate::text::note;

const USAGE: &str = "usage: builds checks run [<worktree>] | builds checks list [<worktree>] | builds checks tests --since <rev> [--run] | builds checks stats [--project <project>] [--since <YYYY-MM-DD>] [--change <rev>]";

pub fn cmd_checks(args: &[String]) -> Result<()> {
    match args {
        [sub] if sub == "run" => hook(&top_of(Path::new("."))?),
        [sub, worktree] if sub == "run" => hook(&top_of(Path::new(worktree))?),
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
    let Some(checkout) = Checkout::at_keeping_env(top)? else { return Ok(None) };
    Ok(project_of(&places, &checkout)?.map(|project| places.lists.join(project).join(hook_stats::DATABASE)))
}

/// `builds checks stats [--project P] [--since DATE] [--change REV]`.
fn stats(args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["since", "change"], &[])?;
    if !args.positional().is_empty() {
        fail!("{USAGE}");
    }
    let since = args.value("since")?;
    let start = match since {
        Some(day) => match crate::text::local_start_of(day) {
            Some(start) => Some(start),
            None => fail!("--since takes a day as YYYY-MM-DD, not {day:?}"),
        },
        None => None,
    };
    let places = Places::from_env()?;
    let store = Store::resolve(&places, &args, Path::new("."))?;
    if let Some(rev) = args.value("change")? {
        if since.is_some() {
            fail!("--change splits all the runs; it does not take --since");
        }
        let impact = hook_impact::impact(&store.dir.join(hook_stats::DATABASE), rev)?;
        print!("{}", hook_impact::report(&store.project, impact.as_ref()));
        return Ok(());
    }
    let stats = hook_stats::stats(&store.dir.join(hook_stats::DATABASE), start.unwrap_or(0))?;
    print!("{}", hook_stats::report(&store.project, since, stats.as_ref()));
    Ok(())
}

fn top_of(dir: &Path) -> Result<PathBuf> {
    let text = git_text(git(dir).args(["rev-parse", "--show-toplevel"]), &format!("find the top of {}", dir.display()))?;
    Ok(PathBuf::from(text.trim()))
}

fn load(top: &Path) -> Result<Vec<Line>> {
    let file = top.join(CONFIG);
    parse(&std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?)
}

fn z_list(text: &str) -> Vec<String> {
    text.split('\0').filter(|name| !name.is_empty()).map(str::to_string).collect()
}

/// `git diff --cached --name-only --diff-filter=ACMR`.
fn staged(top: &Path) -> Result<Vec<String>> {
    let text = git_text(
        git(top).args(["diff", "--cached", "--name-only", "-z", "--diff-filter=ACMR"]),
        "list the staged files",
    )?;
    Ok(z_list(&text))
}

/// Which of <files> have changes in the working tree that are not staged.
fn unstaged_among(top: &Path, files: &[String]) -> Result<Vec<String>> {
    let text = git_text(git(top).args(["diff", "--name-only", "-z", "--"]).args(files), "list the unstaged changes")?;
    Ok(z_list(&text))
}

fn fingerprint(top: &Path, file: &str) -> Option<u64> {
    let bytes = std::fs::read(top.join(file)).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

struct Ran {
    ok: bool,
    took: Duration,
    /// What it printed, stdout then stderr; empty when it streamed.
    output: String,
}

/// Runs <planned> in <top>, keeping its output unless <stream>.
fn execute(top: &Path, planned: &Planned, stream: bool) -> Result<Ran> {
    let (program, args) = planned.argv.split_first().expect("a line has a program");
    let path = if program.contains('/') { top.join(program) } else { PathBuf::from(program) };
    let mut command = Command::new(path);
    command.args(args).current_dir(top).stdin(Stdio::null());
    for name in git_local_env_vars()? {
        command.env_remove(name);
    }
    let started = Instant::now();
    if stream {
        let ok = command.status().is_ok_and(|status| status.success());
        return Ok(Ran { ok, took: started.elapsed(), output: String::new() });
    }
    Ok(match command.output() {
        Ok(output) => Ran {
            ok: output.status.success(),
            took: started.elapsed(),
            output: format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)),
        },
        Err(error) => Ran { ok: false, took: started.elapsed(), output: format!("could not run {program}: {error}\n") },
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
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            fail!("git diff --numstat printed {entry:?}");
        };
        // A binary file has `-` for both counts.
        lines.insert(path.to_string(), added.parse::<u64>().ok().zip(removed.parse::<u64>().ok()));
    }
    let mut names = status.split('\0').filter(|entry| !entry.is_empty());
    let mut files = Vec::new();
    while let Some(letter) = names.next() {
        let Some(path) = names.next() else { fail!("git diff --name-status printed {letter:?} with no path") };
        let change = match letter {
            "A" => Change::Added,
            "D" => Change::Deleted,
            _ => Change::Modified,
        };
        files.push(StagedFile { path: path.to_string(), change, lines: lines.get(path).copied().flatten() });
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
    let output = git(top).args(["rev-parse", "--verify", "-q", "HEAD"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
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
    let started = Instant::now();
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

fn run_checks(top: &Path, started: Instant, mut run: Option<&mut Run>) -> Result<()> {
    let lines = load(top)?;
    let picked = plan(top, &lines, &[Kind::Check, Kind::Fix], &staged(top)?, false)?;
    if let Some(run) = run.as_deref_mut() {
        run.plan_ms = millis(started.elapsed());
        run.checks = lines
            .iter()
            .filter(|line| matches!(line.kind, Kind::Check | Kind::Fix))
            .map(|line| CheckRecord {
                name: line.name.clone(),
                kind: line.kind,
                outcome: Outcome::Skipped,
                handed: None,
                ms: None,
            })
            .collect();
    }
    if picked.is_empty() {
        return Ok(());
    }
    let mut failed: Vec<&str> = Vec::new();
    let mut refusals: Vec<String> = Vec::new();
    for planned in &picked {
        let name = planned.line.name.as_str();
        let before: Vec<(String, Option<u64>)> = match planned.line.kind {
            Kind::Fix => planned.files.iter().map(|file| (file.clone(), fingerprint(top, file))).collect(),
            _ => Vec::new(),
        };
        let unstaged = if before.is_empty() { Vec::new() } else { unstaged_among(top, &planned.files)? };
        let ran = execute(top, planned, false)?;
        let mut problem = (!ran.ok).then(|| ran.output.clone());
        if ran.ok && planned.line.kind == Kind::Fix {
            let changed: Vec<String> =
                before.into_iter().filter(|(file, was)| fingerprint(top, file) != *was).map(|(file, _)| file).collect();
            if let Some(partial) = changed.iter().find(|file| unstaged.contains(file)) {
                problem = Some(String::new());
                refusals.push(format!(
                    "{name} changed {partial}, which has changes that are not staged; staging it would add them to the commit, so nothing was staged. Stage or stash the rest of the file and commit again"
                ));
            } else if !changed.is_empty() {
                git_text(git(top).args(["add", "--"]).args(&changed), "stage the files a fix changed")?;
            }
        }
        if let Some(run) = run.as_deref_mut() {
            let record = run.checks.iter_mut().find(|c| c.name == name).expect("every picked line is recorded");
            record.outcome = if problem.is_none() { Outcome::Passed } else { Outcome::Failed };
            record.handed = Some((
                if planned.line.command.iter().any(|arg| arg == "{files}") { planned.files.len() as u32 } else { 0 },
                if planned.line.command.iter().any(|arg| arg == "{crates}") {
                    planned.argv.iter().filter(|arg| *arg == "-p").count() as u32
                } else {
                    0
                },
            ));
            record.ms = Some(millis(ran.took));
        }
        match problem {
            None => note(format!("{} {name}: ok, {}", planned.line.kind.word(), seconds(ran.took))),
            Some(output) => {
                note(format!("{} {name}: FAILED, {}", planned.line.kind.word(), seconds(ran.took)));
                eprint!("{output}");
                failed.push(name);
            }
        }
    }
    note(format!("{} run, {} failed, {}", picked.len(), failed.len(), seconds(started.elapsed())));
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Fail::Message(format!("failed: {}{}", failed.join(", "), refusals.iter().map(|r| format!("\n{r}")).collect::<String>())))
    }
}

pub fn list_staged(top: &Path) -> Result<String> {
    let lines = load(top)?;
    let picked = plan(top, &lines, &[Kind::Check, Kind::Fix], &staged(top)?, false)?;
    Ok(picked.iter().map(|p| format!("{} {}: {}\n", p.line.kind.word(), p.line.name, p.argv.join(" "))).collect())
}

/// The `test` lines that the files changed in `<since>..HEAD` match, and, when
/// <since> is not a commit here, every test with the reason why.
pub fn select_tests(top: &Path, lines: &[Line], since: &str) -> Result<(Vec<Planned>, Option<String>)> {
    let known = git(top)
        .args(["rev-parse", "--verify", "--quiet", &format!("{since}^{{commit}}")])
        .stdin(Stdio::null())
        .output()
        .context(|| "running git rev-parse".to_string())?
        .status
        .success();
    if known {
        let text = git_text(
            git(top).args(["diff", "--name-only", "-z", "--no-renames", &format!("{since}..HEAD")]),
            "list the changed files",
        )?;
        Ok((plan(top, lines, &[Kind::Test], &z_list(&text), false)?, None))
    } else {
        let everything = z_list(&git_text(git(top).args(["ls-files", "-z"]), "list the files")?);
        let why = format!("{since} is not in the history, so every test runs");
        Ok((plan(top, lines, &[Kind::Test], &everything, true)?, Some(why)))
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
        let ran = execute(top, p, true)?;
        note(format!("test {}: {}, {}", p.line.name, if ran.ok { "ok" } else { "FAILED" }, seconds(ran.took)));
        if !ran.ok {
            failed.push(p.line.name.as_str());
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(Fail::Message(format!("failed: {}", failed.join(", ")))) }
}

#[cfg(test)]
#[path = "checks_run_tests.rs"]
mod tests;
