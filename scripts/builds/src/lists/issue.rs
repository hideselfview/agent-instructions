//! GitHub issues a record may be deferred to. The one question asked of
//! GitHub, whether an issue is open, goes through a runner the caller
//! passes in: `real_gh` runs the `gh` program, tests pass a stub. It runs
//! before a command takes the store's lock, since it is outside work.

use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::fail;
use crate::fail::{Context, Fail, Result};

/// Runs `gh` with these arguments and returns what it printed. Refuses when
/// it fails.
pub type GhRunner<'a> = &'a dyn Fn(&[&str]) -> Result<String>;

/// The real `gh`.
pub fn real_gh(args: &[&str]) -> Result<String> {
    let output = Command::new("gh")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running gh {}", args.join(" ")))?;
    if !output.status.success() {
        fail!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Deserialize)]
struct IssueState {
    state: String,
}

/// <url> names a GitHub issue that is open.
pub fn require_open_issue(gh: GhRunner<'_>, url: &str) -> Result<()> {
    let printed = gh(&["issue", "view", url, "--json", "state"])?;
    let issue: IssueState = serde_json::from_str(&printed)
        .map_err(|error| Fail::Message(format!("gh printed no issue state for {url}: {error}")))?;
    if issue.state != "OPEN" {
        fail!(
            "issue {url} is {}, not open; defer to an open issue (`gh issue create` makes one)",
            issue.state
        );
    }
    Ok(())
}
