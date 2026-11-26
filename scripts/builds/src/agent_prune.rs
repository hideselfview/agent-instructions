//! Prune of the agents' folders on the build volume
//! (`<volume>/agents/<session>/`, the log and the scratch folder): builds
//! deletes a session's folder once every item of the agent's record is closed
//! and Claude Code's record of running sessions no longer lists the session.
//! A folder that no store records an agent for stays, since the command that
//! makes a folder records the agent after it.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::agent_record;
use crate::db;
use crate::fail::Result;
use crate::prune::Run;
use crate::text::note;
use crate::tool::Tool;

/// The session folders of <agents> to delete: those named in <finished>
/// and not in <running>, as paths.
pub(crate) fn deletable(
    agents: &std::path::Path,
    finished: &[String],
    running: &HashSet<std::ffi::OsString>,
) -> Vec<PathBuf> {
    finished
        .iter()
        .filter(|session| !running.contains(std::ffi::OsStr::new(session.as_str())))
        .map(|session| agents.join(session))
        .filter(|folder| folder.is_dir())
        .collect()
}

/// Deletes the folder of each agent that every store records as finished
/// (prune.rs). When builds cannot read a store or the running sessions, it
/// deletes nothing and says so.
pub fn prune(tool: &Tool, run: Run) -> Result<()> {
    let agents = tool.places.agents();
    if !agents.is_dir() {
        return Ok(());
    }
    let running = match crate::prune::live_sessions(&tool.places.claude_sessions()) {
        Ok(running) => running,
        Err(error) => {
            note(format!(
                "reading the running Claude Code sessions failed, so no agent folder was deleted: {error}"
            ));
            return Ok(());
        }
    };
    let mut finished = Vec::new();
    for path in crate::lists::store::database_paths(&tool.places)? {
        let Some(conn) = db::open_reader(&path)? else {
            continue;
        };
        match agent_record::sessions_with_closed_items(&conn) {
            Ok(sessions) => finished.extend(sessions),
            Err(error) => {
                note(format!(
                    "reading the agents of {} failed, so no agent folder was deleted: {}",
                    path.display(),
                    error.text()
                ));
                return Ok(());
            }
        }
    }
    crate::prune::prune_listed(
        run,
        &format!(
            "agent folders of finished agents under {}",
            agents.display()
        ),
        Ok(deletable(&agents, &finished, &running)),
    );
    Ok(())
}

#[cfg(test)]
#[path = "agent_prune_tests.rs"]
mod tests;
