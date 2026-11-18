//! A job's claim on its worktree on a host: the folder `<worktree>.job`,
//! whose file `id` holds the job's id, `<pid>-<nanoseconds>` (`remote::job_id`),
//! where <pid> is the `builds` process on this Mac that runs the job and
//! <nanoseconds> is when it made the id. That process releases the claim when
//! the job ends. A process that is killed, or a Mac that stops, leaves the
//! claim behind, and nothing else would release it, so the claim of a gone
//! process holds nothing: the next job that wants the worktree removes it and
//! claims the worktree (`remote::claim_script`), and `builds prune` removes a
//! worktree that no checkout uses when its claim is such a one
//! (remote_worktrees.rs).
//!
//! Only this Mac can tell whether the process runs, and the scripts that claim
//! a worktree run on the host. So builds reads this Mac's processes once
//! (`Holders`) and hands the scripts what it read, with the time it read them:
//! a claim made after that time is too new to judge, and counts as held.

use std::collections::HashMap;

use crate::fail::Result;
use crate::procs::Processes;

/// Whether the process that made a claim runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    /// It runs, or it made the claim after builds read the processes.
    Runs,
    /// It was gone when builds read the processes.
    Gone,
}

/// This Mac's processes at one moment, as the claims' scripts need them.
#[derive(Clone, Debug)]
pub struct Holders {
    /// When builds read the processes, in nanoseconds since 1970.
    read_at: u128,
    /// The start of each process that ran then, in seconds since 1970, by pid.
    started: HashMap<i32, i64>,
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos()
}

impl Holders {
    /// Reads this Mac's processes. The time comes first, so every process
    /// that made a claim before it and still runs is among them. Fails when
    /// the processes cannot be read, so no claim is judged gone from a short
    /// list.
    pub fn read() -> Result<Holders> {
        let read_at = now_nanos();
        let started = Processes::read()?
            .list
            .iter()
            .map(|process| (process.pid, process.started))
            .collect();
        Ok(Holders { read_at, started })
    }

    /// Whether the process that made job <id>'s claim runs; None when <id> is
    /// not `<pid>-<nanoseconds>`. A process that runs with that pid but
    /// started after the id was made took the pid of the gone one.
    pub fn holder(&self, id: &str) -> Option<Holder> {
        let (pid, made) = id.split_once('-')?;
        let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        if !digits(pid) || !digits(made) {
            return None;
        }
        let (pid, made) = (pid.parse::<u64>().ok()?, made.parse::<u128>().ok()?);
        if made >= self.read_at {
            return Some(Holder::Runs);
        }
        let made_second = (made / 1_000_000_000) as i64;
        let started = i32::try_from(pid)
            .ok()
            .and_then(|pid| self.started.get(&pid));
        Some(match started {
            Some(started) if *started <= made_second => Holder::Runs,
            _ => Holder::Gone,
        })
    }

    /// The shell lines that define `claim_gone`, which judges a claim as
    /// `holder` does, and `release_gone` (`RELEASE_GONE`).
    pub fn script(&self) -> String {
        let holders: Vec<String> = self
            .started
            .iter()
            .map(|(pid, started)| format!("{pid}:{started}"))
            .collect();
        format!(
            "read_at={}\nholders='{}'\n{CLAIM_GONE}{RELEASE_GONE}",
            self.read_at,
            holders.join(" ")
        )
    }
}

/// `claim_gone <claim folder>` prints the job id in the claim and succeeds
/// when the process that made it was gone at `$read_at`, by the `pid:start`
/// pairs in `$holders`, as `Holders::holder` judges. It fails for a claim with
/// no id yet (a job is claiming it now) or an id it cannot read.
const CLAIM_GONE: &str = r#"claim_gone() {
  claim_id=$(cat "$1/id" 2>/dev/null | tr -d ' \n')
  case "$claim_id" in *-*) ;; *) return 1;; esac
  claim_pid=${claim_id%%-*}
  claim_made=${claim_id#*-}
  case "$claim_pid" in ''|*[!0-9]*) return 1;; esac
  case "$claim_made" in ''|*[!0-9]*) return 1;; esac
  [ "$claim_made" -lt "$read_at" ] || return 1
  for claim_holder in $holders; do
    [ "${claim_holder%%:*}" = "$claim_pid" ] &&
      [ "${claim_holder#*:}" -le $((claim_made / 1000000000)) ] && return 1
  done
  echo "$claim_id"
}
"#;

/// `release_gone <claim folder> <id>` removes the claim folder when it still
/// holds job <id>'s claim, and fails otherwise. A job may claim the worktree
/// between the judgment that the claim is gone and its release, and two
/// scripts may release the same claim at once: the folder `taking` inside the
/// claim lets one script at a time read the id and remove the claim.
pub const RELEASE_GONE: &str = r#"release_gone() {
  mkdir "$1/taking" 2>/dev/null || return 1
  if [ "$(cat "$1/id" 2>/dev/null | tr -d ' \n')" = "$2" ]; then
    rm -rf "$1"
    return
  fi
  rmdir "$1/taking"
  return 1
}
"#;

#[cfg(test)]
#[path = "job_claim_tests.rs"]
mod tests;
