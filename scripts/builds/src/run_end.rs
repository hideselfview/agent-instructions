//! How a run that builds started ended: an app build lane's run
//! (app_timing.rs) or a `builds cargo` run (cargo_run_outcome.rs). Both
//! records store it in an `outcome` column that takes the same words.

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunEnd {
    Passed,
    Failed,
    /// A signal ended it, or builds stopped it.
    Stopped,
}

impl RunEnd {
    pub fn word(self) -> &'static str {
        match self {
            RunEnd::Passed => "passed",
            RunEnd::Failed => "failed",
            RunEnd::Stopped => "stopped",
        }
    }

    /// How a run whose program exited with <code> ended: a code above 128 is
    /// a signal's (`fail::exit_code`).
    pub fn of_code(code: i32) -> RunEnd {
        match code {
            0 => RunEnd::Passed,
            129.. => RunEnd::Stopped,
            _ => RunEnd::Failed,
        }
    }
}
