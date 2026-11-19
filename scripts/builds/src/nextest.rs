//! cargo-nextest on a host: the one version builds installs on every host
//! that takes cargo work (`install`, run by `builds install` here and by
//! `builds host setup-here` on each host), and how a probe reads which version
//! a host has (`PROBE_SCRIPT_LINES`, `parse_version`), so that a `builds cargo
//! nextest` run goes only to a host that has it (`host::takes_nextest`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::host::sh_quote;
use crate::text::note;

/// The cargo-nextest version on every host and on this Mac.
pub const VERSION: &str = "0.9.143";

/// Where cargo finds its subcommand programs under <home>.
fn bin_dir(home: &Path) -> PathBuf {
    home.join(".cargo/bin")
}

/// The line a probe's script prints when the host has cargo-nextest: its
/// version, from the first line of `cargo-nextest nextest --version`
/// (`cargo-nextest 0.9.143`). A host without it prints nothing.
pub const PROBE_SCRIPT_LINES: &str = r#"if v=$("$HOME/.cargo/bin/cargo-nextest" nextest --version 2>/dev/null); then echo "nextest=$(printf '%s\n' "$v" | head -n 1 | awk '{print $2}')"; fi"#;

/// The version a probe's answer reports, None when the host has no
/// cargo-nextest.
pub fn parse_version(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("nextest="))
        .map(|version| version.trim().to_string())
        .filter(|version| !version.is_empty())
}

/// The version of the cargo-nextest under <home>'s `.cargo/bin`, None when
/// there is none or it does not run.
pub fn installed_version(home: &Path) -> Option<String> {
    let program = bin_dir(home).join("cargo-nextest");
    if !program.exists() {
        return None;
    }
    let output = Command::new(&program)
        .args(["nextest", "--version"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .map(str::to_string)
        })
        .flatten()
}

/// Installs cargo-nextest `VERSION` under <home>'s `.cargo/bin` when it has
/// another version or none, and returns whether it did. <download> puts the
/// release's `cargo-nextest` program in the folder it is given.
pub fn install_with(home: &Path, download: impl FnOnce(&Path) -> Result<()>) -> Result<bool> {
    if installed_version(home).as_deref() == Some(VERSION) {
        return Ok(false);
    }
    let bin = bin_dir(home);
    std::fs::create_dir_all(&bin).context(|| format!("creating {}", bin.display()))?;
    // A folder beside the program, so that the rename stays on one volume
    // and a run of `cargo nextest` finds a whole program or the old one.
    let part = bin.join(format!(".cargo-nextest.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part).context(|| format!("creating {}", part.display()))?;
    let placed = (|| -> Result<()> {
        download(&part)?;
        let program = part.join("cargo-nextest");
        if !program.exists() {
            fail!("the cargo-nextest {VERSION} download held no cargo-nextest program");
        }
        let target = bin.join("cargo-nextest");
        std::fs::rename(&program, &target).context(|| format!("replacing {}", target.display()))?;
        match installed_version(home).as_deref() {
            Some(VERSION) => Ok(()),
            other => fail!(
                "{} reports version {other:?} after installing {VERSION}",
                target.display()
            ),
        }
    })();
    let _ = std::fs::remove_dir_all(&part);
    placed.map(|()| true)
}

/// Downloads the release's universal macOS build of cargo-nextest into
/// <dir>.
fn download(dir: &Path) -> Result<()> {
    let script = format!(
        "curl -fsSL https://get.nexte.st/{VERSION}/mac | tar xzf - -C {}",
        sh_quote(&dir.to_string_lossy())
    );
    let output = Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .output()
        .context(|| "running curl and tar".to_string())?;
    if !output.status.success() {
        fail!(
            "downloading cargo-nextest {VERSION} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Installs cargo-nextest `VERSION` under <home> (`install_with`), saying so
/// on stderr when it changed anything.
pub fn install(home: &Path) -> Result<()> {
    if install_with(home, download)? {
        note(format!("installed cargo-nextest {VERSION}"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "nextest_tests.rs"]
pub mod tests;
