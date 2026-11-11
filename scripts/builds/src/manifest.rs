//! A repo's `.build-dirs`: its build output paths and the size each may reach,
//! and the shared caches every checkout uses, with theirs.

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;

pub const MANIFEST: &str = ".build-dirs";

/// One build output path of a checkout and its caps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildDir {
    /// Relative to the checkout and to its build root: `target`,
    /// `bae-macos/bae/.build`.
    pub path: String,
    cap_kib: u64,
    /// The cap in the repo's app checkout, where it differs (an `app <cap>`
    /// field): its cargo builds run without incremental compilation (see
    /// `cargo_run`), so a full build there takes less.
    app_cap_kib: Option<u64>,
    /// The app lane that builds into the folder (a `lane <lane>` field): only
    /// that lane's app checkout keeps it warm and takes the app cap. None for
    /// a folder every lane's builds write to (host `target`).
    pub lane: Option<Lane>,
}

impl BuildDir {
    /// Is the folder an app checkout's, one that stays warm: in <checkout_lane>,
    /// the lane the checkout is the app checkout of (None for any other
    /// checkout)? A folder of the other lane is not: nothing builds into it
    /// there.
    pub fn is_app_in(&self, checkout_lane: Option<Lane>) -> bool {
        checkout_lane.is_some_and(|lane| self.lane.is_none_or(|own| own == lane))
    }

    /// The folder's cap in an app checkout (<app>, from `is_app_in`) or in any
    /// other checkout.
    pub fn cap_kib(&self, app: bool) -> u64 {
        match (app, self.app_cap_kib) {
            (true, Some(cap)) => cap,
            _ => self.cap_kib,
        }
    }
}

pub fn has_manifest(top: &Path) -> bool {
    top.join(MANIFEST).is_file()
}

/// The entries of <top>'s manifest, in its order; none without one. Fails on
/// a malformed line, naming it.
pub fn build_dirs(top: &Path) -> Result<Vec<BuildDir>> {
    Ok(parse(top)?.0)
}

/// The paths a `keep <path>` line of <top>'s manifest names: builds inside a
/// build folder (an Xcode tree) that `setup` and `prune` never age out in a
/// checkout that keeps its builds warm (main, the app checkout), because the
/// person runs or tests the app built there. A path is relative to the
/// checkout.
pub fn kept_builds(top: &Path) -> Result<Vec<String>> {
    Ok(parse(top)?.1)
}

fn parse(top: &Path) -> Result<(Vec<BuildDir>, Vec<String>)> {
    let file = top.join(MANIFEST);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(error) => fail!("reading {}: {error}", file.display()),
    };
    let mut entries = Vec::new();
    let mut kept = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let Some(path) = fields.next() else { continue };
        if path.starts_with('#') {
            continue;
        }
        if path == "keep" {
            let (Some(kept_path), None) = (fields.next(), fields.next()) else {
                fail!("{}: 'keep' takes one path, got '{line}'", file.display());
            };
            if kept_path.starts_with('/') || kept_path.split('/').any(|part| part == "..") {
                fail!("{}: '{kept_path}' must be inside the checkout", file.display());
            }
            kept.push(kept_path.to_string());
            continue;
        }
        let cap = fields.next().unwrap_or("");
        let (mut app_cap, mut lane) = (None, None);
        while let Some(field) = fields.next() {
            match (field, fields.next()) {
                ("app", Some(cap)) if app_cap.is_none() => app_cap = Some(cap),
                ("lane", Some(word)) if lane.is_none() => match Lane::parse(word) {
                    Some(parsed) => lane = Some(parsed),
                    None => fail!("{}: '{line}' names the lane '{word}', which is not mac or android", file.display()),
                },
                _ => fail!("{}: '{line}' takes a path, a cap and at most 'app <cap>' and 'lane <lane>'", file.display()),
            }
        }
        if path.starts_with('/')
            || path == ".."
            || path.starts_with("../")
            || path.ends_with("/..")
            || path.contains("/../")
        {
            fail!("{}: '{path}' must be inside the checkout", file.display());
        }
        let what = format!("{}: '{path}'", file.display());
        let cap_kib = cap_kib(cap, &what)?;
        let app_cap_kib = app_cap.map(|cap| self::cap_kib(cap, &format!("{what} (app)"))).transpose()?;
        entries.push(BuildDir { path: path.to_string(), cap_kib, app_cap_kib, lane });
    }
    Ok((entries, kept))
}

/// <cap> (20G, 500M) in KiB; fails naming <what> when it is neither.
pub fn cap_kib(cap: &str, what: &str) -> Result<u64> {
    let (digits, unit) = cap.split_at(cap.len().saturating_sub(1));
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n) = digits.parse::<u64>() {
            match unit {
                "G" => return Ok(n * 1024 * 1024),
                "M" => return Ok(n * 1024),
                _ => {}
            }
        }
    fail!("{what} needs a cap like 20G or 500M, got '{cap}'")
}

/// A download cache every checkout shares: a folder in <root>/caches with a
/// link to it from where its tool looks.
#[derive(Clone, Debug)]
pub struct SharedCache {
    pub name: &'static str,
    pub cap_kib: u64,
    /// Relative to $HOME.
    pub link: &'static str,
    /// Settings the person writes into the tool's home folder, which a
    /// deletion copies out first and puts back in the emptied folder.
    pub settings: &'static [&'static str],
}

impl SharedCache {
    pub fn link_path(&self, home: &Path) -> PathBuf {
        home.join(self.link)
    }
}

const GIB: u64 = 1024 * 1024;

/// The shared caches. Caps are about twice each cache's size when they got
/// them (Oct 2026: gradle 3.4G, npm 0.3G, nuget 2.6G, swiftpm 1.0G). Old
/// Gradle, AGP and NuGet package versions pile up as dependencies move, and
/// deleting a cache costs every checkout a download, so the cap leaves room
/// rather than fitting tightly. sccache's cap is the size `install` writes
/// into its config, and sccache evicts within it itself; its own accounting
/// reads about 0.2 GiB under the folder's size on disk, so status may show
/// it a little over. 30G holds about two days of builds: on a day of agents
/// building in seven bae and five coven checkouts sccache wrote 12.6G, and
/// nothing in it was older than three days.
///
/// The links for Gradle and NuGet point at their tools' whole home folders,
/// which also hold settings the person writes (gradle.properties, init
/// scripts, NuGet.Config).
pub const SHARED_CACHES: &[SharedCache] = &[
    SharedCache {
        name: "gradle",
        cap_kib: 8 * GIB,
        link: ".gradle",
        settings: &["gradle.properties", "init.d", "init.gradle", "init.gradle.kts"],
    },
    SharedCache { name: "npm", cap_kib: GIB, link: ".npm", settings: &[] },
    SharedCache {
        name: "nuget",
        cap_kib: 6 * GIB,
        link: ".nuget",
        settings: &["NuGet/NuGet.Config"],
    },
    SharedCache {
        name: "swiftpm",
        cap_kib: 3 * GIB,
        link: "Library/Caches/org.swift.swiftpm",
        settings: &[],
    },
    SharedCache {
        name: "sccache",
        cap_kib: 30 * GIB,
        link: "Library/Caches/Mozilla.sccache",
        settings: &[],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps() {
        assert_eq!(cap_kib("20G", "x").unwrap(), 20 * GIB);
        assert_eq!(cap_kib("500M", "x").unwrap(), 500 * 1024);
        for bad in ["", "G", "20", "2X", "1G2G", "-1G", "20g"] {
            assert!(cap_kib(bad, "x").is_err(), "{bad} passed");
        }
    }

    #[test]
    fn manifest_lines() {
        let dir = std::env::temp_dir().join(format!("builds-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), "# a comment\n\ntarget 30G app 12G\n  bae/.build   2G  \nlast 1M").unwrap();
        let entries = build_dirs(&dir).unwrap();
        assert_eq!(
            entries,
            vec![
                BuildDir { path: "target".into(), cap_kib: 30 * GIB, app_cap_kib: Some(12 * GIB), lane: None },
                BuildDir { path: "bae/.build".into(), cap_kib: 2 * GIB, app_cap_kib: None, lane: None },
                BuildDir { path: "last".into(), cap_kib: 1024, app_cap_kib: None, lane: None },
            ]
        );
        assert_eq!(entries[0].cap_kib(false), 30 * GIB);
        assert_eq!(entries[0].cap_kib(true), 12 * GIB);
        assert_eq!(entries[1].cap_kib(true), 2 * GIB);
        for (bad, says) in [
            ("target 1G extra\n", "takes a path, a cap and at most 'app <cap>' and 'lane <lane>'"),
            ("target 1G app\n", "takes a path, a cap and at most 'app <cap>' and 'lane <lane>'"),
            ("target 1G app 1G more\n", "takes a path, a cap and at most 'app <cap>' and 'lane <lane>'"),
            ("target 1G app 1X\n", "'target' (app) needs a cap like 20G or 500M, got '1X'"),
            ("../out 1G\n", "must be inside the checkout"),
            ("/abs 1G\n", "must be inside the checkout"),
            ("a/../b 1G\n", "must be inside the checkout"),
            ("target\n", "needs a cap like 20G or 500M, got ''"),
        ] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            match build_dirs(&dir) {
                Err(fail::Fail::Message(message)) => assert!(message.contains(says), "{message}"),
                other => panic!("{bad}: {other:?}"),
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lane_field_names_the_lane_whose_app_checkout_keeps_the_folder_warm() {
        let dir = std::env::temp_dir().join(format!("builds-manifest-lane-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), "target 30G app 12G\nxcode 2G app 1G lane mac\ngradle 2G lane android app 1G\n").unwrap();
        let entries = build_dirs(&dir).unwrap();
        let lanes: Vec<Option<Lane>> = entries.iter().map(|entry| entry.lane).collect();
        assert_eq!(lanes, vec![None, Some(Lane::Mac), Some(Lane::Android)]);
        assert_eq!(entries[2].cap_kib(true), GIB);
        // The host folder is every app checkout's; a lane's folder its lane's.
        assert!(entries[0].is_app_in(Some(Lane::Mac)) && entries[0].is_app_in(Some(Lane::Android)));
        assert!(entries[1].is_app_in(Some(Lane::Mac)) && !entries[1].is_app_in(Some(Lane::Android)));
        assert!(!entries[2].is_app_in(Some(Lane::Mac)) && entries[2].is_app_in(Some(Lane::Android)));
        assert!(!entries[0].is_app_in(None), "another checkout has no app folders");
        for bad in ["x 1G lane ios\n", "x 1G lane\n", "x 1G lane mac lane android\n", "x 1G app 1G app 2G\n"] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            assert!(build_dirs(&dir).is_err(), "{bad} passed");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keep_lines() {
        let dir = std::env::temp_dir().join(format!("builds-manifest-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), "bae/.build 2G\nkeep bae/.build/run\n").unwrap();
        assert_eq!(build_dirs(&dir).unwrap(), vec![BuildDir { path: "bae/.build".into(), cap_kib: 2 * GIB, app_cap_kib: None, lane: None }]);
        assert_eq!(kept_builds(&dir).unwrap(), vec!["bae/.build/run".to_string()]);
        for bad in ["keep\n", "keep a b\n", "keep ../x\n"] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            assert!(kept_builds(&dir).is_err(), "{bad} passed");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
