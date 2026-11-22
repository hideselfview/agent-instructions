//! What each shots step of an app build said of the saved base run and the
//! saved pictures it used (app_build_step_shots.sql): whether the run of the
//! merge base that the step compares with was already saved, and how many of
//! its screenshots came from the picture folder and how many it shot. `builds
//! health` shows the rates (shots_hits.rs).
//!
//! app_build.rs reads each line of a shots step's output as it passes through
//! (`ReusedRead`) and builds each step's `ShotsUse`; app_timing.rs calls
//! `record` in the transaction that writes the run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, Transaction, params};

use crate::app_timing::StepRecord;
use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lock::EachLine;

/// Whether the run of the merge base that a shots step compares with was
/// there for the build to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseRun {
    /// The commit is on origin/main, or the step's prefix starts no shot
    /// declared at the merge base: the step compares with nothing.
    NotNeeded,
    /// The run was saved when the build looked for it.
    Saved,
    /// The run was missing, so the build made it in its own turn, or waited
    /// for the build that was making it.
    Built,
}

impl BaseRun {
    pub fn word(self) -> &'static str {
        match self {
            BaseRun::NotNeeded => "not needed",
            BaseRun::Saved => "saved",
            BaseRun::Built => "built",
        }
    }

    pub fn from_word(word: &str) -> Result<BaseRun> {
        Ok(match word {
            "not needed" => BaseRun::NotNeeded,
            "saved" => BaseRun::Saved,
            "built" => BaseRun::Built,
            other => fail!("a shots step's base run is {other:?}"),
        })
    }
}

/// The screenshots of a finished shots step: those taken from the picture
/// folder and those shot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pictures {
    pub reused: u32,
    pub captured: u32,
}

impl Pictures {
    /// What a finished step took: <reused> screenshots from the picture
    /// folder, or, when it took none from it, all <scenes> it takes shot
    /// again. The checker takes either every selected shot from the folder
    /// or shoots every one, so the two never mix. None when <scenes> is None:
    /// the step's command counts no scenes.
    pub fn of(scenes: Option<usize>, reused: u32) -> Option<Pictures> {
        let scenes = u32::try_from(scenes?).ok()?;
        Some(if reused > 0 {
            Pictures {
                reused,
                captured: 0,
            }
        } else {
            Pictures {
                reused: 0,
                captured: scenes,
            }
        })
    }
}

/// What a shots step used. `pictures` is None for a step that ended before
/// its run finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotsUse {
    /// The host software that drew the step's shots (shots_environment.rs).
    pub environment: String,
    pub base_run: BaseRun,
    pub pictures: Option<Pictures>,
}

/// The line the shots checker prints when a run takes every selected shot of
/// a platform from the picture folder, in two parts around the shot count
/// and the platform: `builds shots: took the 12 mac shots of app tree <tree>
/// from the picture folder; capturing none` (scripts/shots/src/lib.rs
/// `capture`).
const REUSED_START: &str = "builds shots: took the ";
const REUSED_END: &str = " from the picture folder; capturing none";

/// The count of screenshots <line> says the checker took from the picture
/// folder; None for any other line.
pub fn reused_in(line: &str) -> Option<u32> {
    let (_, rest) = line.split_once(REUSED_START)?;
    if !rest.contains(REUSED_END) {
        return None;
    }
    rest.split(' ').next()?.parse().ok()
}

/// Reads each line of a shots step's output, from both streams, and adds up
/// the screenshots it says the checker took from the picture folder. Clones
/// share the count.
#[derive(Clone, Default)]
pub struct ReusedRead(Arc<Mutex<u32>>);

impl ReusedRead {
    /// Reads one line, without its newline.
    pub fn line(&self, text: &str) {
        if let Some(count) = reused_in(text) {
            *self.0.lock().expect("no panic holding it") += count;
        }
    }

    /// The reader a run gives each line of the step's output to.
    pub fn each_line(&self) -> EachLine {
        let read = self.clone();
        Arc::new(move |text: &str| read.line(text))
    }

    /// The screenshots read so far.
    pub fn reused(&self) -> u32 {
        *self.0.lock().expect("no panic holding it")
    }
}

/// Writes what each shots step of <steps> used, as the steps of run
/// <run_id>, by their position.
pub fn record(tx: &Transaction, run_id: i64, steps: &[StepRecord]) -> Result<()> {
    let mut insert = tx
        .prepare(
            "INSERT INTO app_build_step_shots (run_id, position, environment, base_run, reused, captured) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .sql()?;
    for (position, step) in steps.iter().enumerate() {
        let Some(used) = &step.shots else {
            continue;
        };
        insert
            .execute(params![
                run_id,
                position as i64,
                used.environment,
                used.base_run.word(),
                used.pictures.map(|p| p.reused),
                used.pictures.map(|p| p.captured),
            ])
            .sql()?;
    }
    Ok(())
}

/// A stored row: position, environment, base run, reused and captured.
type StoredUse = (u32, String, String, Option<u32>, Option<u32>);

/// What each shots step of run <run_id> used, by its position, when builds
/// kept it.
pub fn read(db: &Connection, run_id: i64) -> Result<HashMap<u32, ShotsUse>> {
    let rows: Vec<StoredUse> = db
        .prepare(
            "SELECT position, environment, base_run, reused, captured FROM app_build_step_shots WHERE run_id = ?1",
        )
        .sql()?
        .query_map([run_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .sql()?
        .collect::<std::result::Result<_, _>>()
        .sql()?;
    rows.into_iter()
        .map(|(position, environment, base_run, reused, captured)| {
            Ok((
                position,
                ShotsUse {
                    environment,
                    base_run: BaseRun::from_word(&base_run)?,
                    pictures: reused
                        .zip(captured)
                        .map(|(reused, captured)| Pictures { reused, captured }),
                },
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "app_build_step_shots_tests.rs"]
mod tests;
