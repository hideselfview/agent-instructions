//! cargo-nextest on a host: the one version builds installs on every host
//! that takes cargo work (`install`, run by `builds install` here and by
//! `builds host setup-here` on each host), and how a probe reads which version
//! a host has (`PROBE_SCRIPT_LINES`, `parse_has`), so that a `builds cargo
//! nextest` run goes only to a host that has it (`host::takes_run`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::text::note;

/// The cargo-nextest version on every host and on this Mac.
pub const VERSION: &str = "0.9.143";

/// Where cargo finds its subcommand programs under <home>.
fn bin_dir(home: &Path) -> PathBuf {
    home.join(".cargo/bin")
}

/// What a host has of cargo-nextest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Has {
    /// No cargo-nextest program.
    Nothing,
    /// A program that reports this version.
    Version(String),
    /// A program that does not start, and what it said.
    Broken(String),
}

/// What a probe's script prints when the host has a cargo-nextest program:
/// `nextest=<version>` from the first line of `cargo-nextest nextest
/// --version` (`cargo-nextest 0.9.143`), or `nextest_fails=<what it said>`
/// when the program does not start. A host without the program prints nothing.
pub const PROBE_SCRIPT_LINES: &str = r#"if [ -e "$HOME/.cargo/bin/cargo-nextest" ]; then if v=$("$HOME/.cargo/bin/cargo-nextest" nextest --version 2>&1); then echo "nextest=$(printf '%s\n' "$v" | head -n 1 | awk '{print $2}')"; else echo "nextest_fails=$(printf '%s' "$v" | tr '\n' ' ')"; fi; fi"#;

/// What a probe's answer says the host has.
pub fn parse_has(text: &str) -> Has {
    if let Some(said) = text
        .lines()
        .find_map(|line| line.strip_prefix("nextest_fails="))
    {
        return Has::Broken(said.trim().to_string());
    }
    match text
        .lines()
        .find_map(|line| line.strip_prefix("nextest="))
        .map(str::trim)
    {
        Some("") => Has::Broken("`cargo-nextest nextest --version` printed no version".into()),
        Some(version) => Has::Version(version.to_string()),
        None => Has::Nothing,
    }
}

/// What the cargo-nextest under <home>'s `.cargo/bin` is.
pub fn has(home: &Path) -> Has {
    let program = bin_dir(home).join("cargo-nextest");
    if !program.exists() {
        return Has::Nothing;
    }
    let output = match Command::new(&program)
        .args(["nextest", "--version"])
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) => return Has::Broken(format!("running {}: {error}", program.display())),
    };
    if !output.status.success() {
        return Has::Broken(format!(
            "{} failed ({}): {}",
            program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    match String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
    {
        Some(version) => Has::Version(version.to_string()),
        None => Has::Broken(format!("{} printed no version", program.display())),
    }
}

/// Installs cargo-nextest `VERSION` under <home>'s `.cargo/bin` when it has
/// another version or none, and returns whether it did. <download> puts the
/// release's `cargo-nextest` program in the folder it is given, a new one
/// beside the program (`evict::in_temp_dir`), so that the rename stays on one volume. Fails when
/// the program there does not start, and when the new folder cannot be
/// created or removed.
pub fn install_with(home: &Path, download: impl FnOnce(&Path) -> Result<()>) -> Result<bool> {
    match has(home) {
        Has::Version(version) if version == VERSION => return Ok(false),
        Has::Broken(said) => fail!(
            "the cargo-nextest under {} does not start: {said}",
            home.display()
        ),
        Has::Version(_) | Has::Nothing => {}
    }
    let bin = bin_dir(home);
    std::fs::create_dir_all(&bin).context(|| format!("creating {}", bin.display()))?;
    crate::evict::in_temp_dir(&bin, ".cargo-nextest.", |part| {
        download(part)?;
        let program = part.join("cargo-nextest");
        if !program.exists() {
            fail!("the cargo-nextest {VERSION} download held no cargo-nextest program");
        }
        let target = bin.join("cargo-nextest");
        std::fs::rename(&program, &target).context(|| format!("replacing {}", target.display()))?;
        match has(home) {
            Has::Version(version) if version == VERSION => Ok(true),
            other => fail!(
                "{} is {other:?} after installing {VERSION}",
                target.display()
            ),
        }
    })
}

/// The name get.nexte.st gives the release's build for this machine: the
/// universal macOS build, or the Linux build for its processor.
fn release_platform() -> &'static str {
    match (crate::host_platform::Os::here(), std::env::consts::ARCH) {
        (crate::host_platform::Os::Mac, _) => "mac",
        (crate::host_platform::Os::Linux, "aarch64") => "linux-arm",
        (crate::host_platform::Os::Linux, _) => "linux",
    }
}

/// Downloads the release's build of cargo-nextest for this machine into
/// <dir>.
fn download(dir: &Path) -> Result<()> {
    crate::download::unpack_tarball(
        &format!("https://get.nexte.st/{VERSION}/{}", release_platform()),
        dir,
        &format!("cargo-nextest {VERSION}"),
    )
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
