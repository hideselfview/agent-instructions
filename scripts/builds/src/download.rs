//! Downloading a release archive of a program that builds installs, such as
//! cargo-nextest and node (cargo_program.rs) and elan's installer (lean.rs).

use std::path::Path;
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::host::sh_quote;

/// Downloads the gzipped tar archive at <url> with curl and unpacks it into
/// <dir>. A failure names <what> and says what curl or tar printed.
pub fn unpack_tarball(url: &str, dir: &Path, what: &str) -> Result<()> {
    unpack(url, dir, what, "")
}

/// Downloads the gzipped tar archive at <url> and puts only its file <member>
/// in <dir>, under the member's own file name.
pub fn unpack_member(url: &str, member: &str, dir: &Path, what: &str) -> Result<()> {
    let depth = member.matches('/').count();
    unpack(
        url,
        dir,
        what,
        &format!("--strip-components={depth} {}", sh_quote(member)),
    )
}

fn unpack(url: &str, dir: &Path, what: &str, tar_args: &str) -> Result<()> {
    let script = format!(
        "curl -fsSL {} | tar xzf - -C {} {tar_args}",
        sh_quote(url),
        sh_quote(&dir.to_string_lossy())
    );
    let output = Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .output()
        .context(|| "running curl and tar".to_string())?;
    if !output.status.success() {
        fail!(
            "downloading {what} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;
