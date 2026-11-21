//! The findings of `--shots` runs: what builds reads from a shots run's
//! output to cache it in the shots cache (shots_cache.rs), and the commands that
//! print a cached run. A branch's run compares with its before run, the cached
//! run of its merge base with origin/main (`shots_cache::before_run`), so that
//! run is read from the cache and not built again, and an agent that wants a
//! commit's findings reads what an earlier run printed.
//!
//! A run is cached under the repo, the full commit id, the platform, the prefix
//! the run was given and the version of the shots checker (`VERSION` in
//! shots_program.rs: a hash of its sources, which each run's findings.json
//! names). A run by another version of the checker has another key, so its
//! findings never stand in for these, and builds caches no run whose
//! findings.json names another version than its own.
//!
//! A run is cached when the shots command exits 0, or exits another status
//! after printing that pairs fail the run; a command that died on an error
//! (a build that failed, a simulator that did not start) has no findings and
//! caches nothing. What is cached is what the shots program printed, from the
//! line that starts it (cargo's `Running` line) to the end, whole.
//!
//! The run's report and pictures are cached with the findings, so the next run
//! in the same app checkout (which rewrites the checkout's `shots/` folder)
//! cannot take them. When the shots program prints `<its name>: report at
//! <folder>/index.html`, the run copies that `index.html`, the files it names
//! (`img/...`) and the findings as records (`findings.json`, which
//! shot_causes.rs reads) from the checkout, or from a host's checkout, into
//! the run's folder in the cache. The run prints the local path of the report
//! when it ends, and `builds shots findings` prints it again.
//!
//! `builds shots findings <repo> <ref> <platform> <prefix>` prints a cached
//! run, and with `--by-cause` its findings grouped by cause, from the records
//! its report holds (shot_causes.rs). `builds app-build <repo> <ref>
//! --shots <prefix>` (or, in its lane form, `--shots <platform> <prefix>` with
//! no build step) prints it too, in place of taking the lane's lock, unless
//! the command has `--fresh`. A run of
//! a platform's full set of shots (`--all-shots <platform>`) is cached under the
//! prefix every shot of the platform starts with (`mac.`).

use std::path::Path;

use crate::app_build;
use crate::app_timing::now;
use crate::fail;
use crate::fail::{Context, Result};
use crate::host::sh_quote;
use crate::lane::Lane;
use crate::places::Places;
use crate::shot_causes;
use crate::shots_cache::{self, Cached, Key};
use crate::text::note;

/// The line the shots program's own output starts after: cargo's, for the
/// command it runs.
const PROGRAM_STARTS: &str = "Running `";

/// What a failing run prints when pairs fail it, as bae-shots words it: the
/// sign that the run made findings and did not die first.
const PAIRS_FAIL: &str = "fail the run";

/// What the shots program prints, after its own name (`bae-shots`, `builds
/// shots`), before the path of its report.
const REPORT_AT: &str = ": report at ";

/// The folder of the shots report <output> names (the folder that holds the
/// `index.html` its last `<program>: report at` line gives), if it does.
pub fn report_dir_in(output: &str) -> Option<String> {
    let line = output
        .lines()
        .rev()
        .find_map(|line| line.trim().split_once(REPORT_AT).map(|(_, path)| path))?;
    let dir = line.trim().strip_suffix("/index.html")?;
    (!dir.is_empty()).then(|| dir.to_string())
}

/// The files, relative to the report's folder, that the report page <html>
/// shows: each `src="..."` that is a plain relative path.
pub fn report_files(html: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for part in html.split("src=\"").skip(1) {
        let Some((path, _)) = part.split_once('"') else {
            continue;
        };
        let plain = !path.is_empty()
            && !path.starts_with('/')
            && !path.contains(':')
            && !path.split('/').any(|piece| piece == "..");
        if plain && !files.iter().any(|known| known == path) {
            files.push(path.to_string());
        }
    }
    files
}

/// Copies the report in <from> (its `index.html`, the files that page shows
/// and its findings as records, when the shots program wrote them) into the
/// new folder <into>.
pub fn copy_report(from: &Path, into: &Path) -> Result<()> {
    let page = from.join("index.html");
    let html = std::fs::read_to_string(&page).context(|| format!("reading {}", page.display()))?;
    let copy = |name: &str| -> Result<()> {
        let to = into.join(name);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::copy(from.join(name), &to)
            .map(|_| ())
            .context(|| format!("copying {name} from {}", from.display()))
    };
    copy("index.html")?;
    for name in report_files(&html) {
        copy(&name)?;
    }
    if from.join(shot_causes::FILE).is_file() {
        copy(shot_causes::FILE)?;
    }
    Ok(())
}

/// The shell script that writes to its output a tar of the report in <dir>:
/// its `index.html`, <files> (the files the page shows) and its findings as
/// records when the folder has them. A copy from a host's checkout runs it
/// there.
pub fn report_copy_script(dir: &str, files: &[String]) -> String {
    let named: Vec<String> = std::iter::once("index.html")
        .chain(files.iter().map(String::as_str))
        .map(sh_quote)
        .collect();
    let records = sh_quote(shot_causes::FILE);
    format!(
        "cd {} && set -- {} && if [ -f {records} ]; then set -- \"$@\" {records}; fi && tar -chf - \"$@\"",
        sh_quote(dir),
        named.join(" ")
    )
}

/// Says where <cached>'s report is, when the run cached one.
pub fn note_report(cached: &Cached) {
    let page = cached.report().join("index.html");
    if page.exists() {
        note(format!(
            "shots report for {}: {}",
            cached.key.prefix,
            page.display()
        ));
    }
}

/// What the shots program printed in <output>: the text after the line that
/// starts it, or all of it when no such line is there.
pub fn findings_of(output: &str) -> &str {
    let mut start = 0;
    for line in output.split_inclusive('\n') {
        let end = start + line.len();
        if line.trim_start().starts_with(PROGRAM_STARTS) {
            return &output[end..];
        }
        start = end;
    }
    output
}

/// What the shots program prints before the names of the pairs that fail the
/// run: `builds shots: 2 pair(s) fail the run: mac.a.b, mac.a.c`.
const FAILING_PAIRS: &str = "pair(s) fail the run: ";

/// The names of the shots that <line> of a shots run's output says fail the
/// run; None for any other line.
pub fn failing_shots(line: &str) -> Option<Vec<String>> {
    let (_, names) = line.split_once(FAILING_PAIRS)?;
    Some(
        names
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// What the shots program prints after the count of the pairs the base never
/// rendered: `builds shots: 2 pair(s) the base never rendered, with 3
/// finding(s) that fail nothing: mac.a.b, mac.a.c`.
const UNRENDERED_PAIRS: &str = "pair(s) the base never rendered, with ";

/// Whether <line> of a shots run's output is the one that says the base never
/// rendered some of its pairs.
pub fn says_unrendered(line: &str) -> bool {
    line.contains(UNRENDERED_PAIRS)
}

/// Whether a run that exited <exit> printed findings: it passed, or said that
/// pairs fail the run.
pub fn has_findings(exit: i64, findings: &str) -> bool {
    exit == 0 || findings.contains(PAIRS_FAIL)
}

/// The key of a run of <prefix> on <lane> for <repo> at <git_ref> by this
/// builds' shots checker: the commit is read from the repo's app checkout.
pub fn key_for(
    places: &Places,
    repo: &str,
    git_ref: &str,
    lane: Lane,
    prefix: &str,
) -> Result<Key> {
    let (_, commit, _) = app_build::commit_and_config(places, repo, git_ref)?;
    Ok(Key {
        repo: repo.to_string(),
        version: crate::shots_program::VERSION.to_string(),
        commit,
        lane,
        prefix: prefix.to_string(),
    })
}

/// The version of the shots checker that the findings.json in the report folder
/// <folder> names.
pub fn version_in(folder: &Path) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Named {
        version: String,
    }
    let file = folder.join(shot_causes::FILE);
    let text = std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
    let named: Named = serde_json::from_str(&text)
        .context(|| format!("{} names no shots checker version", file.display()))?;
    Ok(named.version)
}

/// Prints <cached>: a line to say which run it is, then its text.
pub fn print(cached: &Cached) {
    note_run(cached);
    print!("{}", cached.findings);
}

/// Says on stderr which run <cached> is: its commit, lane and prefix, how old
/// it is, the shots program's version and the exit status.
fn note_run(cached: &Cached) {
    let Key {
        repo,
        commit,
        lane,
        prefix,
        version,
    } = &cached.key;
    note(format!(
        "{repo} {} {} {prefix}: findings of a run that ended {} ago (shots program version {}), which exited {}",
        &commit[..commit.len().min(12)],
        lane.word(),
        age(now() - cached.recorded_at),
        &version[..version.len().min(12)],
        cached.exit_code
    ));
}

/// The findings of the cached run <cached> grouped by cause, from the records
/// its report holds. Fails when the run cached no such records, naming the
/// command that runs it again for <git_ref>.
pub fn by_cause(cached: &Cached, git_ref: &str) -> Result<String> {
    match shot_causes::read(&cached.report()) {
        Ok(run) => Ok(shot_causes::text(&run)),
        Err(error) => fail!(
            "{}. The shots program writes its findings as records beside its report, which builds caches with the run; `{} --fresh` runs this one again",
            error.text(),
            how_to_make(
                &cached.key.repo,
                git_ref,
                cached.key.lane,
                &cached.key.prefix
            )
        ),
    }
}

fn age(seconds: i64) -> String {
    match seconds.max(0) {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

const USAGE: &str = "usage: builds shots findings <repo> <ref> mac|android <prefix> [--by-cause]";

/// The command a missing run of <prefix> on <lane> is made with: the
/// lane's full set of shots for the prefix every shot of the lane starts
/// with.
pub fn how_to_make(repo: &str, git_ref: &str, lane: Lane, prefix: &str) -> String {
    if prefix == lane.all_shots_prefix() {
        format!(
            "builds app-build {repo} {git_ref} {} {}",
            app_build::ALL_SHOTS,
            lane.word()
        )
    } else {
        format!("builds app-build {repo} {git_ref} --shots {prefix}")
    }
}

/// `builds shots findings`, given the arguments after `findings`.
pub fn cmd_findings(places: &Places, args: &[String]) -> Result<()> {
    let grouped = args.iter().any(|arg| arg == "--by-cause");
    let args: Vec<&String> = args.iter().filter(|arg| *arg != "--by-cause").collect();
    let [repo, git_ref, lane, prefix] = args[..] else {
        fail!("{USAGE}");
    };
    let lane = Lane::parse_or_fail(lane)?;
    let key = key_for(places, repo, git_ref, lane, prefix)?;
    match shots_cache::lookup(places, &key)? {
        Some(cached) if grouped => {
            let text = by_cause(&cached, git_ref)?;
            note_run(&cached);
            print!("{text}");
            note_report(&cached);
            Ok(())
        }
        Some(cached) => {
            print(&cached);
            note_report(&cached);
            Ok(())
        }
        None => fail!(
            "no run of {prefix} on {} is cached for {repo} at {} (shots program version {}); `{}` makes one",
            lane.word(),
            &key.commit[..12.min(key.commit.len())],
            &key.version[..12.min(key.version.len())],
            how_to_make(repo, git_ref, lane, prefix)
        ),
    }
}

#[cfg(test)]
#[path = "shot_findings_tests.rs"]
mod tests;
