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
//! line that starts it (cargo's `Running` line) to the end.
//!
//! `builds shots findings <repo> <ref> <platform> <prefix>` prints a stored
//! run. `builds app-build <repo> <ref> --shots <prefix>` (or, in its lane form,
//! `--shots <platform> <prefix>` with no build step) prints it too, in
//! place of taking the lane's lock, unless the command has `--fresh`.

use std::path::Path;

use rusqlite::params;

use crate::app_build;
use crate::app_config::CONFIG;
use crate::app_timing::{self, now};
use crate::db::{self, Sql};
use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;
use crate::places::Places;
use crate::text::note;

/// The line the shots program's own output starts after: cargo's, for the
/// command it runs.
const PROGRAM_STARTS: &str = "Running `";

/// What a failing run prints when pairs fail it, as bae-shots words it: the
/// sign that the run made findings and did not die first.
const PAIRS_FAIL: &str = "fail the run";

/// The most of a run's text that is kept: its end.
const KEEP: usize = 1 << 20;

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
/// starts it, or all of it when no such line is there; its last `KEEP` bytes.
pub fn findings_of(output: &str) -> &str {
    let mut start = 0;
    for line in output.split_inclusive('\n') {
        let end = start + line.len();
        if line.trim_start().starts_with(PROGRAM_STARTS) {
            return tail(&output[end..]);
        }
        start = end;
    }
    tail(output)
}

fn tail(text: &str) -> &str {
    if text.len() <= KEEP {
        return text;
    }
    let mut from = text.len() - KEEP;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    &text[from..]
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
    print!("{}", stored.findings);
}

fn age(seconds: i64) -> String {
    match seconds.max(0) {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

const USAGE: &str = "usage: builds shots findings <repo> <ref> mac|android <prefix>";

/// The command a missing run is made with.
pub fn how_to_make(repo: &str, git_ref: &str, prefix: &str) -> String {
    format!("builds app-build {repo} {git_ref} --shots {prefix}")
}

pub fn cmd_shots(places: &Places, args: &[String]) -> Result<()> {
    let [sub, rest @ ..] = args else {
        fail!("{USAGE}");
    };
    if sub != "findings" {
        fail!("unknown shots command {sub}\n{USAGE}");
    }
    let [repo, git_ref, lane, prefix] = rest else {
        fail!("{USAGE}");
    };
    let lane = Lane::parse_or_fail(lane)?;
    let (key, database) = key_for(places, repo, git_ref, lane, prefix)?;
    let stored = match &database {
        Some(database) => lookup(database, &key)?,
        None => None,
    };
    match stored {
        Some(stored) => {
            print(&stored);
            Ok(())
        }
        None => fail!(
            "no run of {prefix} on {} is stored for {repo} at {} (shots program version {}); `{}` makes one",
            lane.word(),
            &key.commit[..12.min(key.commit.len())],
            &key.version[..12.min(key.version.len())],
            how_to_make(repo, git_ref, prefix)
        ),
    }
}

#[cfg(test)]
#[path = "shot_findings_tests.rs"]
mod tests;
