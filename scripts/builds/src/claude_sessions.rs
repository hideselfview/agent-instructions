//! Claude Code's record of its running sessions: one `<pid>.json` in
//! `~/.claude/sessions` for each Claude Code process, naming the session it
//! runs. Every command a session runs, in the foreground or as a background
//! task, is a child of that process, and the session waits on it until the
//! command ends or the session stops it. A shell whose parent has a record
//! here is therefore one a live session waits on.

use std::collections::HashSet;
use std::path::Path;

/// A Claude Code process that is still running.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub pid: i32,
    pub session_id: String,
}

/// The sessions of <dir> whose process is alive. A pid that a later process
/// took still counts as a running session; callers only use that to keep
/// something. An unreadable record is an error, never skipped, so no caller
/// takes a short list for all the sessions.
pub fn running(dir: &Path) -> Result<Vec<Running>, String> {
    #[derive(serde::Deserialize)]
    struct Registered {
        pid: i32,
        #[serde(rename = "sessionId")]
        session_id: String,
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
        if crate::procs::runs(registered.pid) {
            live.push(Running {
                pid: registered.pid,
                session_id: registered.session_id,
            });
        }
    }
    Ok(live)
}

/// The pids of <dir>'s running sessions.
pub fn pids(dir: &Path) -> Result<HashSet<i32>, String> {
    Ok(running(dir)?.into_iter().map(|r| r.pid).collect())
}
