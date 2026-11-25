//! Claude Code's record of its running sessions: one `<pid>.json` in
//! `~/.claude/sessions` for each Claude Code process, naming the session it
//! runs. Every command a session runs, in the foreground or as a background
//! task, is a child of that process, and the session waits on it until the
//! command ends or the session stops it. A shell whose parent has a record
//! here is therefore one a live session waits on.

use std::path::Path;

use crate::procs::Processes;

/// A Claude Code process that is still running.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Running {
    pub pid: i32,
    pub session_id: String,
}

/// The sessions of <dir> whose process <processes> lists. A record whose pid a
/// later process took, one that started after the record did, names no
/// session. An unreadable record is an error, never skipped, so no caller
/// takes a short list for all the sessions.
pub fn running(dir: &Path, processes: &Processes) -> Result<Vec<Running>, String> {
    #[derive(serde::Deserialize)]
    struct Registered {
        pid: i32,
        #[serde(rename = "sessionId")]
        session_id: String,
        /// Milliseconds since 1970.
        #[serde(rename = "startedAt")]
        started_at: i64,
    }
    let mut live = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(live),
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{}: {error}", dir.display()))?
            .path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            // A session that ended between the listing and the read.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let registered: Registered = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        // A pid that <processes> lacks has exited, started after the list was
        // read, or belongs to another user: no session of this one.
        let Some(process) = processes.get(registered.pid) else {
            continue;
        };
        // The record is written after the process starts.
        if process.started <= registered.started_at / 1000 {
            live.push(Running {
                pid: registered.pid,
                session_id: registered.session_id,
            });
        }
    }
    Ok(live)
}

/// The sessions of <dir> that run now, from one reading of the processes.
pub fn running_now(dir: &Path) -> Result<Vec<Running>, String> {
    let processes = Processes::read().map_err(|error| error.text())?;
    running(dir, &processes)
}

#[cfg(test)]
#[path = "claude_sessions_tests.rs"]
mod tests;
