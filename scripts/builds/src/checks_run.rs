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
//! git runs here with the environment the hook has, so it reads and writes the
//! index the commit uses. The programs get none of git's own variables: a
//! linked worktree's `GIT_DIR` breaks SwiftPM's package clones.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::checkout::{git, git_local_env_vars, git_text};
use crate::checks::{CONFIG, Kind, Line, Planned, parse, plan};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::text::note;

const USAGE: &str = "usage: builds checks run [<worktree>] | builds checks list [<worktree>] | builds checks tests --since <rev> [--run]";

pub fn cmd_checks(args: &[String]) -> Result<()> {
    match args {
        [sub] if sub == "run" => run_staged(&top_of(Path::new("."))?),
        [sub, worktree] if sub == "run" => run_staged(&top_of(Path::new(worktree))?),
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

pub fn run_staged(top: &Path) -> Result<()> {
    let started = Instant::now();
    let lines = load(top)?;
    let picked = plan(top, &lines, &[Kind::Check, Kind::Fix], &staged(top)?, false)?;
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
