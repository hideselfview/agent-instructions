//! What the build that holds an app lock is doing, kept beside the lock in
//! `app.locks/<checkout>.run` for health and the dashboard to read: the
//! build's process, when it joined the lane's queue, how long it waited there
//! in all, and the step it runs now with when that step started, or the step
//! a host holds and what holds it (host_display.rs `Wait`). The holder
//! writes the record whole when it takes the lock and again as each step
//! starts (lock.rs `Held`). A record that names another process than the
//! lock's holder is a past holder's, and readers leave it out.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fail;
use crate::fail::{Context, Result};
use crate::text::minutes_since;

/// The record a lock's holder keeps of its run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderRun {
    /// The process that holds the lock and wrote the record.
    pub pid: i32,
    /// When the build joined the lane's queue, seconds since 1970.
    pub joined_at: i64,
    /// How long the build waited in the lane's queue in all, a wait after a
    /// takeover included.
    pub waited_ms: u64,
    /// The step it runs now; None before its first step and for a holder
    /// that runs no steps (`builds lock`, `build-app`).
    pub step: Option<RunningStep>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningStep {
    pub name: String,
    /// Seconds since 1970: when the step started, or, while a host holds it,
    /// when the hold began.
    pub started_at: i64,
    /// What holds the step before it starts; None once it runs.
    pub waits: Option<crate::host_display::Wait>,
}

/// How long a holder has waited and built, at one moment, in whole minutes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTimes {
    /// When the build took its checkout or started its latest step.
    pub moved_at: i64,
    /// How long it waited in the lane's queue in all.
    pub waited_minutes: i64,
    /// How long it has built since it joined the queue, leaving out its
    /// waits.
    pub building_minutes: i64,
    pub step: Option<StepTime>,
}

/// The step a holder runs now and how long it has run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepTime {
    pub name: String,
    pub minutes: i64,
    /// What holds the step before it starts, for those minutes.
    pub waits: Option<crate::host_display::Wait>,
}

impl HolderRun {
    /// The time the build took its checkout, in seconds since 1970.
    pub fn checkout_at(&self) -> i64 {
        self.joined_at + (self.waited_ms / 1000) as i64
    }

    /// Its times at <now>, seconds since 1970.
    pub fn times(&self, now: i64) -> RunTimes {
        let waited = (self.waited_ms / 1000) as i64;
        RunTimes {
            moved_at: self
                .step
                .as_ref()
                .map_or_else(|| self.checkout_at(), |step| step.started_at),
            waited_minutes: waited / 60,
            building_minutes: minutes_since(now, self.checkout_at()),
            step: self.step.as_ref().map(|step| StepTime {
                name: step.name.clone(),
                minutes: minutes_since(now, step.started_at),
                waits: step.waits.clone(),
            }),
        }
    }

    /// Writes the record to <file> whole.
    pub fn write(&self, file: &Path) -> Result<()> {
        crate::whole_file::write(
            file,
            serde_json::to_vec(self).expect("a holder's record serializes"),
        )
    }

    /// The record at <file>; None when there is none.
    pub fn read(file: &Path) -> Result<Option<HolderRun>> {
        let bytes = match std::fs::read(file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => fail!("reading {}: {error}", file.display()),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .context(|| format!("reading {}", file.display()))
    }
}

#[cfg(test)]
#[path = "holder_run_tests.rs"]
mod tests;
