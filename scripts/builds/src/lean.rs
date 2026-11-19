//! Lean on each Mac that builds a checkout: elan, Lean's toolchain manager,
//! and the toolchain each of the checkout's Lean packages pins.
//!
//! A Lean package of a checkout is a folder whose `.lake` build folder the
//! checkout's `.build-dirs` lists and which holds a `lean-toolchain` file, the
//! toolchain lake builds the package with (`leanprover/lean4:v4.34.1`).
//! `builds setup` puts elan and those toolchains in place under the Mac's
//! `~/.elan` (`ensure`), on this Mac and on a host before each job, so any
//! Mac can build the checkout's Lean packages. elan downloads a toolchain once
//! per Mac; every checkout shares it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::host::sh_quote;
use crate::places::Places;
use crate::text::note;

/// The elan version builds installs on a Mac that has no elan. A Mac that
/// has another version keeps it: elan only fetches and selects toolchains,
/// and each package pins its own toolchain.
pub const ELAN_VERSION: &str = "4.2.4";

/// The file in a Lean package that names its toolchain.
const TOOLCHAIN_FILE: &str = "lean-toolchain";

/// The build folder lake writes in a Lean package.
const LAKE_FOLDER: &str = ".lake";

/// A Lean package of a checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    /// The package's folder, relative to the checkout (`crates/sqlive/proofs`),
    /// or `.` for the checkout's top.
    pub dir: String,
    /// What the package's `lean-toolchain` names.
    pub toolchain: String,
}

/// The Lean packages of the checkout at <top>: each folder whose `.lake` the
/// checkout's `.build-dirs` lists and which holds a `lean-toolchain` file. A
/// listed `.lake` folder without that file belongs to no pinned package and
/// adds none.
pub fn packages(top: &Path) -> Result<Vec<Package>> {
    let mut found = Vec::new();
    for entry in crate::manifest::build_dirs(top)? {
        let dir = if entry.path == LAKE_FOLDER {
            "."
        } else {
            match entry.path.strip_suffix(&format!("/{LAKE_FOLDER}")) {
                Some(dir) => dir,
                None => continue,
            }
        };
        let file = top.join(dir).join(TOOLCHAIN_FILE);
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => fail!("reading {}: {error}", file.display()),
        };
        let toolchain = text.trim();
        if toolchain.is_empty() || toolchain.contains(char::is_whitespace) {
            fail!(
                "{} names no toolchain as one word: {text:?}",
                file.display()
            );
        }
        found.push(Package {
            dir: dir.to_string(),
            toolchain: toolchain.to_string(),
        });
    }
    Ok(found)
}

/// elan's home under <home>, where it keeps its programs and toolchains.
fn elan_home(home: &Path) -> PathBuf {
    home.join(".elan")
}

/// The `lake` that elan installs under <home>, which runs the toolchain the
/// package it runs in pins.
pub fn lake(home: &Path) -> PathBuf {
    elan_home(home).join("bin/lake")
}

fn elan(home: &Path) -> PathBuf {
    elan_home(home).join("bin/elan")
}

/// A command that runs <program>, one of elan's, with elan's home under
/// <home>.
pub fn elan_command(home: &Path, program: &Path) -> Command {
    let mut command = Command::new(program);
    command.env("ELAN_HOME", elan_home(home));
    command
}

/// Puts elan and the toolchain of each Lean package of the checkout at <top>
/// in place on this Mac (`ensure_toolchains`). A checkout with no Lean
/// package installs nothing.
pub fn ensure(places: &Places, top: &Path) -> Result<()> {
    let mut wanted: Vec<String> = packages(top)?
        .into_iter()
        .map(|package| package.toolchain)
        .collect();
    wanted.sort();
    wanted.dedup();
    if wanted.is_empty() {
        return Ok(());
    }
    ensure_toolchains(&places.home, &places.caches(), &wanted, download_elan)
}

/// Installs elan under <home> when it has none, then each toolchain of
/// <wanted> that elan lacks, saying on stderr what it installs. It finishes at
/// once when everything is in place. One Mac installs under the lock in
/// <locks> one at a time, so two jobs that start together on a host install
/// each piece once. <download> puts elan's installer, `elan-init`, in the
/// empty folder it is given.
pub fn ensure_toolchains(
    home: &Path,
    locks: &Path,
    wanted: &[String],
    download: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if missing(home, wanted)?.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(locks).context(|| format!("creating {}", locks.display()))?;
    let _one_at_a_time = crate::evict::lock_exclusive_creating(&locks.join("elan.lock"))?;
    if !elan(home).exists() {
        install_elan(home, download)?;
        note(format!("installed elan {ELAN_VERSION}"));
    }
    for toolchain in missing(home, wanted)? {
        let status = elan_command(home, &elan(home))
            .args(["toolchain", "install", &toolchain])
            .stdin(Stdio::null())
            .stdout(crate::text::stderr_for_child()?)
            .status()
            .context(|| format!("running {} toolchain install", elan(home).display()))?;
        if !status.success() {
            fail!("elan could not install the Lean toolchain {toolchain} ({status})");
        }
        note(format!("installed the Lean toolchain {toolchain}"));
    }
    match missing(home, wanted)?.as_slice() {
        [] => Ok(()),
        still => fail!(
            "elan installed the Lean toolchains {} and still does not list them",
            still.join(", ")
        ),
    }
}

/// The toolchains of <wanted> that elan under <home> does not list: all of
/// them when <home> has no elan.
fn missing(home: &Path, wanted: &[String]) -> Result<Vec<String>> {
    if !elan(home).exists() {
        return Ok(wanted.to_vec());
    }
    let output = elan_command(home, &elan(home))
        .args(["toolchain", "list"])
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running {} toolchain list", elan(home).display()))?;
    if !output.status.success() {
        fail!(
            "{} toolchain list failed ({}): {}",
            elan(home).display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let listed = String::from_utf8_lossy(&output.stdout);
    // Each line is a toolchain, the default one followed by ` (default)`.
    let installed: Vec<&str> = listed
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    Ok(wanted
        .iter()
        .filter(|toolchain| !installed.contains(&toolchain.as_str()))
        .cloned()
        .collect())
}

/// Runs elan's installer under <home> with no default toolchain and no change
/// to the shell's profile, from a new folder beside elan's home that
/// <download> fills and that is removed afterwards.
fn install_elan(home: &Path, download: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    std::fs::create_dir_all(home).context(|| format!("creating {}", home.display()))?;
    let part = crate::evict::make_temp_dir(home, ".elan-init.")?;
    let installed = (|| -> Result<()> {
        download(&part)?;
        let installer = part.join("elan-init");
        let status = elan_command(home, &installer)
            .args(["-y", "--no-modify-path", "--default-toolchain", "none"])
            .stdin(Stdio::null())
            .stdout(crate::text::stderr_for_child()?)
            .status()
            .context(|| format!("running {}", installer.display()))?;
        if !status.success() {
            fail!("elan's installer failed ({status})");
        }
        if !elan(home).exists() {
            fail!(
                "elan's installer finished and {} is missing",
                elan(home).display()
            );
        }
        Ok(())
    })();
    let removed = std::fs::remove_dir_all(&part).context(|| format!("removing {}", part.display()));
    match (installed, removed) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(also)) => fail!("{}; {}", error.text(), also.text()),
    }
}

/// Downloads elan `ELAN_VERSION`'s installer for this Mac's processor into
/// <dir>.
fn download_elan(dir: &Path) -> Result<()> {
    let url = format!(
        "https://github.com/leanprover/elan/releases/download/v{ELAN_VERSION}/elan-{}-apple-darwin.tar.gz",
        std::env::consts::ARCH
    );
    let script = format!(
        "curl -fsSL {} | tar xzf - -C {}",
        sh_quote(&url),
        sh_quote(&dir.to_string_lossy())
    );
    let output = Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .output()
        .context(|| "running curl and tar".to_string())?;
    if !output.status.success() {
        fail!(
            "downloading elan {ELAN_VERSION} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "lean_tests.rs"]
mod tests;
