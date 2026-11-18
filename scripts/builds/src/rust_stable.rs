//! Queuing the move to each new stable Rust release. A project whose repos
//! pin a Rust toolchain (`rust-toolchain.toml` or `rust-toolchain` on the
//! repo's origin/main) gets one queue item when a stable release is newer
//! than a pin, naming each repo's pin and the new version. The dashboard
//! daemon reads rustup's channel manifest every CHECK_HOURS hours and writes
//! the items; a project's store records the release each item was made for
//! (lists/rust_release.rs), so no release makes a second item in a project,
//! however often the manifest is read or the daemon restarts.
//!
//! A manifest that can't be fetched or read, a repo whose pin can't be read
//! and a store that refuses the write are reported in the daemon's log; the
//! daemon tries again after RETRY_HOURS.

use std::fmt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::item::{Audience, ItemDraft};
use crate::lists::store::{Store, stores};
use crate::lists::{Stamp, area, rust_release};
use crate::places::Places;
use crate::text::note;

/// How often the daemon reads the manifest.
pub const CHECK_HOURS: u64 = 6;
/// How long the daemon waits after a check that reported a fault.
pub const RETRY_HOURS: u64 = 1;

/// Rustup's manifest of the stable channel.
const MANIFEST_URL: &str = "https://static.rust-lang.org/dist/channel-rust-stable.toml";

/// Who the lists name for an item made here.
const BY: &str = "builds (stable Rust check)";

/// The areas an item made here goes in, the first the project has; a project
/// with neither uses its first area.
const AREAS: [&str; 2] = ["tooling", "hosts"];

/// A Rust release number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u32, u32, u32);

impl Version {
    /// <text> as exactly `major.minor.patch`; None for a channel name such as
    /// `stable` or a dated nightly.
    pub fn parse(text: &str) -> Option<Version> {
        let mut parts = text.split('.');
        let mut number = || {
            let part = parts.next()?;
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        };
        let version = Version(number()?, number()?, number()?);
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The stable release that <manifest>, rustup's `channel-rust-stable.toml`,
/// names: its `pkg.rust.version` is `1.95.0 (59807616e 2026-04-14)`.
pub fn stable_version(manifest: &str) -> Result<Version> {
    let table: toml::Table = match manifest.parse() {
        Ok(table) => table,
        Err(error) => fail!("the stable channel manifest is not TOML: {error}"),
    };
    let text = table
        .get("pkg")
        .and_then(|pkg| pkg.get("rust"))
        .and_then(|rust| rust.get("version"))
        .and_then(toml::Value::as_str);
    let Some(text) = text else {
        fail!("the stable channel manifest has no pkg.rust.version");
    };
    match text.split_whitespace().next().and_then(Version::parse) {
        Some(version) => Ok(version),
        None => fail!(
            "the stable channel manifest's pkg.rust.version {text:?} starts with no x.y.z version"
        ),
    }
}

/// Fetches the stable channel manifest.
fn fetch_manifest() -> Result<String> {
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--max-time", "60", MANIFEST_URL])
        .stdin(Stdio::null())
        .output()
        .context(|| "starting curl".to_string())?;
    if !output.status.success() {
        fail!(
            "fetching {MANIFEST_URL}: curl {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .map_err(|_| Fail::Message(format!("{MANIFEST_URL} sent text that is not UTF-8")))
}

/// A version a repo pins: `x.y.z`, or `x.y`, which rustup takes as the
/// newest patch release of that minor release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pinned(u32, u32, Option<u32>);

impl Pinned {
    /// Is this pin older than the release <stable>? A pin without a patch
    /// number compares by its two parts.
    pub fn older_than(self, stable: Version) -> bool {
        match self.2 {
            Some(patch) => Version(self.0, self.1, patch) < stable,
            None => (self.0, self.1) < (stable.0, stable.1),
        }
    }
}

impl fmt::Display for Pinned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.2 {
            Some(patch) => write!(f, "{}.{}.{patch}", self.0, self.1),
            None => write!(f, "{}.{}", self.0, self.1),
        }
    }
}

/// A repo's pinned Rust version: the file that pins it and the version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub file: &'static str,
    pub version: Pinned,
}

/// What toolchain channel <text> pins, read as rustup reads it: a channel
/// name (`stable`, `beta` or `nightly`, with a date after it or without)
/// pins no version, and `x.y` and `x.y.z` pin one. Anything else is an error.
fn pinned_version(text: &str) -> std::result::Result<Option<Pinned>, String> {
    let is_date = |rest: &str| {
        let parts: Vec<&str> = rest.split('-').collect();
        matches!(parts.as_slice(), [y, m, d] if [(y, 4), (m, 2), (d, 2)]
            .iter()
            .all(|(part, len)| part.len() == *len && part.bytes().all(|b| b.is_ascii_digit())))
    };
    for channel in ["stable", "beta", "nightly"] {
        if let Some(rest) = text.strip_prefix(channel)
            && (rest.is_empty() || rest.strip_prefix('-').is_some_and(is_date))
        {
            return Ok(None);
        }
    }
    let numbers: Option<Vec<u32>> = text
        .split('.')
        .map(|part| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        })
        .collect();
    match numbers.as_deref() {
        Some([major, minor]) => Ok(Some(Pinned(*major, *minor, None))),
        Some([major, minor, patch]) => Ok(Some(Pinned(*major, *minor, Some(*patch)))),
        _ => Err(format!("channel {text:?} is no channel name, x.y or x.y.z")),
    }
}

/// The version that the repo at <clone> pins on its origin/main, or None when
/// it pins none (no toolchain file, no channel, or a channel name). Like
/// rustup, `rust-toolchain` comes before `rust-toolchain.toml`. A file that
/// can't be read is an error that names it.
pub fn pin(clone: &Path) -> Result<Option<Pin>> {
    for file in ["rust-toolchain", "rust-toolchain.toml"] {
        let listed =
            crate::app_config::run_git(clone, &["ls-tree", "--name-only", "origin/main", file])?;
        if listed.trim().is_empty() {
            continue;
        }
        let text = crate::app_config::run_git(clone, &["show", &format!("origin/main:{file}")])?;
        let channel = pinned_channel(file, &text)
            .map_err(|error| Fail::Message(format!("{file}: {error}")))?;
        let version = match channel.as_deref().map(pinned_version) {
            Some(Ok(version)) => version,
            Some(Err(error)) => fail!("{file}: {error}"),
            None => None,
        };
        return Ok(version.map(|version| Pin { file, version }));
    }
    Ok(None)
}

/// The channel that toolchain file <file> holding <text> names, if it names one.
fn pinned_channel(file: &str, text: &str) -> std::result::Result<Option<String>, String> {
    // The older `rust-toolchain` file may hold only the channel's name.
    let legacy =
        file == "rust-toolchain" && !text.contains(['[', '=']) && text.trim().lines().count() <= 1;
    if legacy {
        return Ok(Some(text.trim().to_string()));
    }
    match text.parse::<toml::Table>() {
        Ok(table) => Ok(table
            .get("toolchain")
            .and_then(|toolchain| toolchain.get("channel"))
            .and_then(toml::Value::as_str)
            .map(str::to_string)),
        Err(error) => Err(format!("is not TOML: {error}")),
    }
}

/// The repos of a project that pin a version older than <stable>, with their pins.
type Lagging = Vec<(String, Pin)>;

/// The item for <stable>: it names each lagging repo's pin.
fn draft(area: String, stable: Version, lagging: &Lagging) -> ItemDraft {
    let pins: Vec<String> = lagging
        .iter()
        .map(|(repo, pin)| format!("{repo} pins {} in {}", pin.version, pin.file))
        .collect();
    ItemDraft {
        title: format!("Move the Rust toolchain pin to {stable}"),
        area,
        details: format!(
            "Rust {stable} is the new stable release (rustup's channel manifest). {}. \
             Move each pin to {stable}.",
            pins.join("; ")
        ),
        r#for: Audience::Coordinator,
        waits: Vec::new(),
        brief: None,
        priority: None,
        tags: Vec::new(),
    }
}

/// Makes the item for <stable> in the store of <store>, if its repos pin an
/// older version and the project made no item for <stable> yet. Returns what
/// it made, in words.
fn queue_for(places: &Places, store: &Store, stable: Version, now: i64) -> Result<Option<String>> {
    let mut lagging = Lagging::new();
    for repo in store.repos()? {
        let clone = places.home.join(crate::host::clone_path(&repo));
        if !clone.is_dir() {
            fail!(
                "{repo} has no clone at {} on this Mac, so its Rust pin is unread",
                clone.display()
            );
        }
        let found = pin(&clone).map_err(|error| {
            Fail::Message(format!("reading the Rust pin of {repo}: {}", error.text()))
        })?;
        if let Some(found) = found.filter(|found| found.version.older_than(stable)) {
            lagging.push((repo, found));
        }
    }
    if lagging.is_empty() {
        return Ok(None);
    }
    let stamp = Stamp {
        at: now,
        by: BY.to_string(),
    };
    let words = format!("rust stable {stable}");
    let made = store.write(&stamp, &words, |db| {
        let areas = area::areas(db)?;
        let Some(area) = AREAS
            .iter()
            .find(|name| areas.iter().any(|known| known == *name))
            .map(|name| name.to_string())
            .or_else(|| areas.first().cloned())
        else {
            fail!("the project has no area to put the item in");
        };
        rust_release::queue_once(
            db,
            &stable.to_string(),
            &draft(area, stable, &lagging),
            &stamp,
        )
    })?;
    Ok(made.map(|id| {
        format!(
            "{}: queued item {id} to move the Rust pin to {stable}",
            store.project
        )
    }))
}

/// Queues the move to <manifest>'s stable release in every project whose
/// repos pin an older one. Returns each item made and each fault: a
/// project that faults does not stop the others.
pub fn check(places: &Places, manifest: &str, now: i64) -> Result<Outcome> {
    let stable = stable_version(manifest)?;
    let mut outcome = Outcome::default();
    for store in stores(places)? {
        match queue_for(places, &store, stable, now) {
            Ok(Some(made)) => outcome.queued.push(made),
            Ok(None) => {}
            Err(error) => outcome
                .faults
                .push(format!("{}: {}", store.project, error.text())),
        }
    }
    Ok(outcome)
}

/// What one check did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub queued: Vec<String>,
    pub faults: Vec<String>,
}

/// Reads the manifest and checks it, saying in the daemon's log what it made
/// and what failed; true when nothing failed.
fn run_once(places: &Places) -> bool {
    let result =
        fetch_manifest().and_then(|manifest| check(places, &manifest, crate::app_timing::now()));
    match result {
        Ok(outcome) => {
            for made in &outcome.queued {
                note(made);
            }
            for fault in &outcome.faults {
                note(format!("stable Rust check: {fault}"));
            }
            outcome.faults.is_empty()
        }
        Err(error) => {
            note(format!("stable Rust check: {}", error.text()));
            false
        }
    }
}

/// Starts the daemon's thread that checks for a new stable release every
/// CHECK_HOURS hours, the first check at once.
pub fn start(places: Places) -> Result<()> {
    std::thread::Builder::new()
        .name("builds-rust-stable".into())
        .spawn(move || {
            loop {
                let hours = if run_once(&places) {
                    CHECK_HOURS
                } else {
                    RETRY_HOURS
                };
                std::thread::sleep(Duration::from_secs(hours * 3600));
            }
        })
        .context(|| "starting a thread".to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "rust_stable_tests.rs"]
mod tests;
