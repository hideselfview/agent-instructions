//! `builds shots check-program`: shows what a change to the shots program
//! (scripts/shots, which builds runs as `builds shots`) clears and adds on a
//! lane's shots, before the change lands. An app build runs the installed
//! shots program, so a branch's change never reaches it; this command builds
//! the branch's program in the checkout it is given, with `builds cargo`, and
//! never installs it. It runs that program over the report of a cached run
//! (shots_cache.rs), which keeps each scene's board and shot pictures with
//! their element lists, and prints the program's findings beside the run's
//! by scene.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::shot_findings;
use crate::shots_cache::Cached;
use crate::text::{note, plural};

const USAGE: &str =
    "usage: builds shots check-program <repo> <ref> <app> <prefix> [--checkout <folder>]";

/// The program a built checkout holds, relative to the checkout's top.
const PROGRAM: &str = "scripts/builds/target/release/builds";

/// The folder of the checkout's builds program, where cargo runs.
const PROGRAM_CRATE: &str = "scripts/builds";

/// The fields the program comparison reads from either version's report.
#[derive(Deserialize)]
struct Run {
    scenes: Vec<String>,
    findings: Vec<FindingText>,
}

#[derive(Deserialize)]
struct FindingText {
    scene: String,
    level: String,
    sentence: String,
}

/// What the compared program made of a scene's findings that the cached run
/// did not, and what it no longer makes.
#[derive(Debug, Default, PartialEq, Eq)]
struct Change {
    /// Each `level: sentence` the cached run has and the program does not.
    clears: Vec<String>,
    /// Each `level: sentence` the program makes and the cached run lacks.
    adds: Vec<String>,
}

/// The scenes whose records differ between <cached> and <program>, with how.
/// A scene's records count each sentence as many times as it appears.
fn changes(cached: &Run, program: &Run) -> BTreeMap<String, Change> {
    fn count(records: &Run) -> BTreeMap<(&str, String), usize> {
        let mut counts = BTreeMap::new();
        for record in &records.findings {
            let line = format!("{}: {}", record.level, record.sentence);
            *counts.entry((record.scene.as_str(), line)).or_insert(0) += 1;
        }
        counts
    }
    let (had, has) = (count(cached), count(program));
    let mut changes: BTreeMap<String, Change> = BTreeMap::new();
    for ((scene, line), was) in &had {
        let now = has.get(&(*scene, line.clone())).copied().unwrap_or(0);
        for _ in now..*was {
            changes
                .entry(scene.to_string())
                .or_default()
                .clears
                .push(line.clone());
        }
    }
    for ((scene, line), now) in &has {
        let was = had.get(&(*scene, line.clone())).copied().unwrap_or(0);
        for _ in was..*now {
            changes
                .entry(scene.to_string())
                .or_default()
                .adds
                .push(line.clone());
        }
    }
    changes
}

/// The text that says, scene by scene, what the program clears and adds.
fn text(cached: &Run, program: &Run) -> String {
    let changes = changes(cached, program);
    let mut out = String::new();
    for (scene, change) in &changes {
        out.push_str(&format!("{scene}\n"));
        for line in &change.clears {
            out.push_str(&format!("  clears {line}\n"));
        }
        for line in &change.adds {
            out.push_str(&format!("  adds   {line}\n"));
        }
    }
    let cleared: usize = changes.values().map(|c| c.clears.len()).sum();
    let added: usize = changes.values().map(|c| c.adds.len()).sum();
    out.push_str(&format!(
        "{cleared} cleared and {added} added, in {} of the {} the run checked\n",
        plural(changes.len(), "scene"),
        plural(cached.scenes.len(), "scene"),
    ));
    out
}

/// Runs <program>, a builds program, as `builds shots recheck` over the
/// report of <cached>, and returns the findings.json it prints.
pub(crate) fn recheck(program: &Path, cached: &Cached) -> Result<String> {
    let ran = Command::new(program)
        .args(["shots", "recheck", "--report"])
        .arg(cached.report())
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running {} shots recheck", program.display()))?;
    if !ran.status.success() {
        fail!(
            "{} shots recheck failed ({}):\n{}",
            program.display(),
            ran.status,
            String::from_utf8_lossy(&ran.stderr).trim_end()
        );
    }
    String::from_utf8(ran.stdout).context(|| "reading the findings the program printed".to_string())
}

/// The text that compares the findings of the cached run with those that
/// <program> makes of the same pictures and element lists.
fn compare(cached: &Cached, program: &Path) -> Result<String> {
    let file = cached.findings_json();
    let before =
        std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
    let before = serde_json::from_str(&before).context(|| format!("reading {}", file.display()))?;
    let after = serde_json::from_str(&recheck(program, cached)?)
        .context(|| "reading the findings the program printed".to_string())?;
    Ok(text(&before, &after))
}

/// Builds the builds program of the checkout <top> in release mode, through
/// `builds cargo`, and returns its path. It installs nothing.
fn build(top: &Path) -> Result<PathBuf> {
    let me = std::env::current_exe().context(|| "finding this program".to_string())?;
    let built = Command::new(me)
        .args(["cargo", "build", "--release"])
        .current_dir(top.join(PROGRAM_CRATE))
        .stdin(Stdio::null())
        .stdout(crate::text::stderr_for_child()?)
        .status()
        .context(|| "running builds cargo build".to_string())?;
    if !built.success() {
        fail!(
            "building the shots program in {} failed ({built})",
            top.display()
        );
    }
    Ok(top.join(PROGRAM))
}

/// The top folder of the git checkout that holds <folder>.
fn top_of(folder: &Path) -> Result<PathBuf> {
    match Checkout::at(folder)? {
        Some(checkout) => Ok(checkout.top),
        None => fail!("{} is in no git checkout", folder.display()),
    }
}

/// `builds shots check-program`, given the arguments after it.
pub fn cmd(places: &Places, args: &[String]) -> Result<()> {
    let (mut named, mut checkout) = (Vec::new(), None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--checkout" {
            let Some(folder) = rest.next() else {
                fail!("{USAGE}");
            };
            checkout = Some(PathBuf::from(folder));
        } else {
            named.push(arg);
        }
    }
    let [repo, git_ref, lane, prefix] = named[..] else {
        fail!("{USAGE}");
    };
    let lane = crate::repo_lanes::parse(places, repo, lane)?;
    let me = crate::this_program()?;
    let cached = shot_findings::cached_run(places, repo, git_ref, lane, prefix, &me)?;
    let here = std::env::current_dir().context(|| "reading the working directory".to_string())?;
    let top = top_of(&checkout.unwrap_or(here))?;
    if !top.join(PROGRAM_CRATE).join("Cargo.toml").is_file() {
        fail!(
            "{} holds no builds program ({PROGRAM_CRATE})",
            top.display()
        );
    }
    let program = build(&top)?;
    let report = compare(&cached, &program)?;
    note(format!(
        "{repo} {} {} {prefix}: findings of the cached run made in environment {} (shots program comparison version {}) beside those of the program built in {}",
        &cached.key.commit[..cached.key.commit.len().min(12)],
        lane.word(),
        cached.key.environment,
        &cached.key.versions.comparison[..cached.key.versions.comparison.len().min(12)],
        top.display()
    ));
    print!("{report}");
    Ok(())
}

#[cfg(test)]
#[path = "shots_check_program_tests.rs"]
mod tests;
