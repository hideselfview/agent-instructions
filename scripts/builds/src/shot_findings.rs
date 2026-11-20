//! The findings of `--shots` runs, kept in the repo's project store
//! (shot_findings.sql) so that an agent that wants the "before" side of a
//! shots check for a commit reads what an earlier run of that commit printed
//! and does not build it again.
//!
//! A row is keyed by the repo, the full commit id, the platform, the prefix
//! the run was given and the version of the shots program: the id of the git
//! tree of the folder `.app-build`'s `shots-source` line names, at that
//! commit. A run by another version of the checker has another key, so its
//! findings never stand in for these. A repo with no `shots-source` line
//! stores and reads nothing.
//!
//! A run is stored when the shots command exits 0, or exits another status
//! after printing that pairs fail the run; a command that died on an error
//! (a build that failed, a simulator that did not start) has no findings and
//! stores nothing. What is stored is what the shots program printed, from the
//! line that starts it (cargo's `Running` line) to the end, whole.
//!
//! The run's report and pictures are kept beside the findings, so the next
//! run in the same app checkout (which rewrites the checkout's `shots/`
//! folder) cannot take them. When the shots program prints `<its name>: report
//! at <folder>/index.html`, the run copies that `index.html`, the files it
//! names (`img/...`) and the findings as records (`findings.json`, which
//! shot_causes.rs reads) from the checkout, or from a host's checkout, to
//! `<store folder>/shots/<commit>-<platform>-<prefix>-<version>/`: the folder
//! of the run's key, replaced by a later run of the same key, and kept for
//! only the newest `KEEP_REPORTS` runs. The run prints the local path of the
//! report when it ends, and `builds shots findings` prints it again.
//!
//! `builds shots findings <repo> <ref> <platform> <prefix>` prints a stored
//! run, and with `--by-cause` its findings grouped by cause, from the records
//! its kept report holds (shot_causes.rs). `builds app-build <repo> <ref>
//! --shots <prefix>` (or, in its lane form, `--shots <platform> <prefix>` with
//! no build step) prints it too, in place of taking the lane's lock, unless
//! the command has `--fresh`. A run of
//! a platform's full set of shots (`--all-shots <platform>`) is kept under the
//! prefix every shot of the platform starts with (`mac.`).

use std::path::Path;

use rusqlite::params;

use crate::app_build;
use crate::app_config::CONFIG;
use crate::app_timing::{self, now};
use crate::db::{self, Sql};
use crate::fail;
use crate::fail::Result;
use crate::host::sh_quote;
use crate::lane::Lane;
use crate::places::Places;
use crate::shot_causes;
use crate::text::note;

/// The line the shots program's own output starts after: cargo's, for the
/// command it runs.
const PROGRAM_STARTS: &str = "Running `";

/// What a failing run prints when pairs fail it, as bae-shots words it: the
/// sign that the run made findings and did not die first.
const PAIRS_FAIL: &str = "fail the run";

/// What a stored run is found by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub repo: String,
    /// The full commit id.
    pub commit: String,
    pub lane: Lane,
    pub prefix: String,
    pub version: String,
}

/// How many runs' report folders a store keeps: the newest by their time.
pub const KEEP_REPORTS: usize = 30;

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

/// The folder that keeps the report of the run <key> in the store whose
/// database is <database>.
pub fn report_folder(database: &Path, key: &Key) -> std::path::PathBuf {
    let word = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("shots")
        .join(format!(
            "{}-{}-{}-{}",
            &key.commit[..key.commit.len().min(12)],
            key.lane.word(),
            word(&key.prefix),
            &key.version[..key.version.len().min(12)]
        ))
}

/// The folder next to <into> that a copy fills before it replaces <into>.
pub fn staging_of(into: &Path) -> std::path::PathBuf {
    let mut name = into.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    into.with_file_name(name)
}

/// Makes <staging> the folder <into>, replacing what is there.
pub fn install_folder(staging: &Path, into: &Path) -> Result<()> {
    if into.exists() {
        std::fs::remove_dir_all(into)
            .map_err(|e| crate::fail::Fail::Message(format!("removing {}: {e}", into.display())))?;
    }
    std::fs::rename(staging, into).map_err(|e| {
        crate::fail::Fail::Message(format!(
            "moving {} to {}: {e}",
            staging.display(),
            into.display()
        ))
    })
}

/// Copies the report in <from> (its `index.html`, the files that page shows
/// and its findings as records, when the shots program wrote them) to the
/// folder <into>.
pub fn copy_report(from: &Path, into: &Path) -> Result<()> {
    let page = from.join("index.html");
    let html = std::fs::read_to_string(&page)
        .map_err(|e| crate::fail::Fail::Message(format!("reading {}: {e}", page.display())))?;
    let staging = staging_of(into);
    let _ = std::fs::remove_dir_all(&staging);
    let copy = |name: &str| -> Result<()> {
        let to = staging.join(name);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                crate::fail::Fail::Message(format!("creating {}: {e}", parent.display()))
            })?;
        }
        std::fs::copy(from.join(name), &to)
            .map(|_| ())
            .map_err(|e| {
                crate::fail::Fail::Message(format!("copying {name} from {}: {e}", from.display()))
            })
    };
    std::fs::create_dir_all(&staging)
        .map_err(|e| crate::fail::Fail::Message(format!("creating {}: {e}", staging.display())))?;
    copy("index.html")?;
    for name in report_files(&html) {
        copy(&name)?;
    }
    if from.join(shot_causes::FILE).is_file() {
        copy(shot_causes::FILE)?;
    }
    install_folder(&staging, into)
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

/// Deletes the report folders in <shots> beyond the newest `KEEP_REPORTS`.
pub fn prune_reports(shots: &Path) {
    let Ok(entries) = std::fs::read_dir(shots) else {
        return;
    };
    let mut folders: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.0));
    for (_, folder) in folders.into_iter().skip(KEEP_REPORTS) {
        let _ = std::fs::remove_dir_all(folder);
    }
}

/// Says where <key>'s stored report is, when its folder is there.
pub fn note_report(database: &Path, key: &Key) {
    let page = report_folder(database, key).join("index.html");
    if page.exists() {
        note(format!(
            "shots report for {}: {}",
            key.prefix,
            page.display()
        ));
    }
}

/// A stored run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub key: Key,
    pub recorded_at: i64,
    pub exit_code: i64,
    pub findings: String,
}

/// The git tree id of <source> at <commit> in the checkout <top>.
pub fn version_at(top: &Path, commit: &str, source: &str) -> Result<String> {
    Ok(crate::app_config::run_git(
        top,
        &["rev-parse", "--verify", &format!("{commit}:{source}")],
    )
    .map_err(|_| {
        crate::fail::Fail::Message(format!(
            "{commit} has no {source} folder, which {CONFIG}'s shots-source line names"
        ))
    })?
    .trim()
    .to_string())
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

/// Whether a run that exited <exit> printed findings: it passed, or said that
/// pairs fail the run.
pub fn has_findings(exit: i64, findings: &str) -> bool {
    exit == 0 || findings.contains(PAIRS_FAIL)
}

/// Writes <findings> of a run that exited <exit> under <key> in the database
/// at <database>, replacing a run with the same key.
pub fn store(database: &Path, key: &Key, exit: i64, findings: &str) -> Result<()> {
    let conn = db::open(database, db::Make::Never)?;
    conn.execute(
        "INSERT OR REPLACE INTO shot_findings (repo, commit_id, platform, prefix, shots_version, recorded_at, exit_code, findings) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            key.repo,
            key.commit,
            key.lane.word(),
            key.prefix,
            key.version,
            now(),
            exit,
            findings
        ],
    )
    .sql()?;
    Ok(())
}

/// The run stored under <key> in the database at <database>, if any.
pub fn lookup(database: &Path, key: &Key) -> Result<Option<Stored>> {
    let Some(conn) = db::open_reader(database)? else {
        return Ok(None);
    };
    let mut statement = conn
        .prepare("SELECT recorded_at, exit_code, findings FROM shot_findings WHERE repo = ?1 AND commit_id = ?2 AND platform = ?3 AND prefix = ?4 AND shots_version = ?5")
        .sql()?;
    let mut rows = statement
        .query(params![
            key.repo,
            key.commit,
            key.lane.word(),
            key.prefix,
            key.version
        ])
        .sql()?;
    match rows.next().sql()? {
        Some(row) => Ok(Some(Stored {
            key: key.clone(),
            recorded_at: row.get(0).sql()?,
            exit_code: row.get(1).sql()?,
            findings: row.get(2).sql()?,
        })),
        None => Ok(None),
    }
}

/// The key of a run of <prefix> on <lane> for <repo> at <git_ref>, and the
/// repo's database: the commit and the version are read from the repo's app
/// checkout. Fails when the repo has no `shots-source` line.
pub fn key_for(
    places: &Places,
    repo: &str,
    git_ref: &str,
    lane: Lane,
    prefix: &str,
) -> Result<(Key, Option<std::path::PathBuf>)> {
    let (top, commit, config) = app_build::commit_and_config(places, repo, git_ref)?;
    let Some(source) = config.shots_source else {
        fail!("{CONFIG} has no shots-source line, so {repo} keeps no shot findings");
    };
    let key = Key {
        repo: repo.to_string(),
        version: version_at(&top, &commit, &source)?,
        commit,
        lane,
        prefix: prefix.to_string(),
    };
    Ok((key, app_timing::database_of(places, repo)?))
}

/// Prints <stored>: a line to say which run it is, then its text.
pub fn print(stored: &Stored) {
    note_run(stored);
    print!("{}", stored.findings);
}

/// Says on stderr which run <stored> is: its commit, lane and prefix, how old
/// it is, the shots program's version and the exit status.
fn note_run(stored: &Stored) {
    let Key {
        repo,
        commit,
        lane,
        prefix,
        version,
    } = &stored.key;
    note(format!(
        "{repo} {} {} {prefix}: findings of a run that ended {} ago (shots program version {}), which exited {}",
        &commit[..commit.len().min(12)],
        lane.word(),
        age(now() - stored.recorded_at),
        &version[..version.len().min(12)],
        stored.exit_code
    ));
}

/// The findings of the run <key> grouped by cause, from the records its kept
/// report holds in the store whose database is <database>. Fails when the
/// store keeps no such records, naming the command that runs <key> again for
/// <git_ref>.
pub fn by_cause(database: &Path, key: &Key, git_ref: &str) -> Result<String> {
    match shot_causes::read(&report_folder(database, key)) {
        Ok(run) => Ok(shot_causes::text(&run)),
        Err(error) => fail!(
            "{}. The shots program writes its findings as records beside its report, and builds keeps the reports of the newest {KEEP_REPORTS} runs; `{} --fresh` runs this one again",
            error.text(),
            how_to_make(&key.repo, git_ref, key.lane, &key.prefix)
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
    let (key, database) = key_for(places, repo, git_ref, lane, prefix)?;
    let stored = match &database {
        Some(database) => lookup(database, &key)?.map(|stored| (database, stored)),
        None => None,
    };
    match stored {
        Some((database, stored)) if grouped => {
            let text = by_cause(database, &stored.key, git_ref)?;
            note_run(&stored);
            print!("{text}");
            note_report(database, &stored.key);
            Ok(())
        }
        Some((database, stored)) => {
            print(&stored);
            note_report(database, &stored.key);
            Ok(())
        }
        None => fail!(
            "no run of {prefix} on {} is stored for {repo} at {} (shots program version {}); `{}` makes one",
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
