//! A repo's `.build-dirs`: its build output paths and the size each may reach,
//! the tools its scripts install into the user's cache home, the programs that
//! prepare a checkout for its builds, and the shared caches every checkout
//! uses, with their sizes.

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
    Ok(parse(top)?.dirs)
}

/// The paths a `keep <path>` line of <top>'s manifest names: builds inside a
/// build folder (an Xcode tree) that `setup` and `prune` never age out in a
/// checkout that keeps its builds warm (main, the app checkout), because the
/// person runs or tests the app built there. A path is relative to the
/// checkout.
pub fn kept_builds(top: &Path) -> Result<Vec<String>> {
    Ok(parse(top)?.kept)
}

/// A tool a repo's script installs once per version into a folder in the
/// user's cache home, for every checkout on the machine to share, with each
/// version in an entry named after it (bae's ownership checker, one per coven
/// revision); and the file in the checkout whose text names the version the
/// checkout runs. An `installed <folder> <file>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// Relative to the cache home: `owner-construction-check`.
    pub folder: String,
    /// Relative to the checkout: `scripts/owner-construction-check.sh`.
    pub file: String,
}

/// The tools the `installed` lines of <top>'s manifest name; none without
/// one.
pub fn installed(top: &Path) -> Result<Vec<Installed>> {
    Ok(parse(top)?.installed)
}

/// The programs the `prepare <program>` lines of <top>'s manifest name, in
/// its order, each relative to the checkout; none without one. Each puts in
/// place what the checkout's builds need beyond their folders, such as a
/// toolchain component or a tool built from the checkout, and finishes at
/// once when all of it is in place (setup.rs runs them).
pub fn prepare_programs(top: &Path) -> Result<Vec<String>> {
    Ok(parse(top)?.prepare)
}

/// A program a repo builds from its own sources for its toolchain, which
/// builds keeps one build of per key on each Mac (built_tools.rs). A `tool
/// <name> <program> <link>` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltTool {
    /// The tool's name, shared by every repo that builds it: `typed-lints`.
    pub name: String,
    /// Relative to the checkout: the program that prints the checkout's key
    /// and builds the tool (`tools/typed-lints/tool.sh`).
    pub program: String,
    /// Relative to the checkout: the link builds points at the key's build
    /// (`tools/typed-lints/built`).
    pub link: String,
}

/// The tools the `tool` lines of <top>'s manifest name; none without one.
pub fn built_tools(top: &Path) -> Result<Vec<BuiltTool>> {
    Ok(parse(top)?.tools)
}

/// The Debian packages the `linux-package` lines of <top>'s manifest name,
/// in its order: what the repo's builds need on a Linux host beyond what
/// builds itself installs there (host_linux.rs); none without one.
pub fn linux_packages(top: &Path) -> Result<Vec<String>> {
    Ok(parse(top)?.linux_packages)
}

/// The cargo packages the `macos` lines of <top>'s manifest name: packages
/// part of whose code compiles only for macOS, so a cargo job that builds one
/// on Linux checks other code than on a Mac, and `builds cargo` runs it on a
/// Mac (cargo_needs.rs); none without one.
pub fn macos_packages(top: &Path) -> Result<Vec<String>> {
    Ok(parse(top)?.macos_packages)
}

/// What a manifest's lines say, in its order.
struct Manifest {
    dirs: Vec<BuildDir>,
    kept: Vec<String>,
    installed: Vec<Installed>,
    prepare: Vec<String>,
    tools: Vec<BuiltTool>,
    linux_packages: Vec<String>,
    macos_packages: Vec<String>,
}

/// Does relative <path> lead out of the folder it is relative to?
fn escapes(path: &str) -> bool {
    path.starts_with('/') || path.split('/').any(|part| part == "..")
}

fn parse(top: &Path) -> Result<Manifest> {
    let file = top.join(MANIFEST);
    let mut manifest = Manifest {
        dirs: Vec::new(),
        kept: Vec::new(),
        installed: Vec::new(),
        prepare: Vec::new(),
        tools: Vec::new(),
        linux_packages: Vec::new(),
        macos_packages: Vec::new(),
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(manifest),
        Err(error) => fail!("reading {}: {error}", file.display()),
    };
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
            if escapes(kept_path) {
                fail!(
                    "{}: '{kept_path}' must be inside the checkout",
                    file.display()
                );
            }
            manifest.kept.push(kept_path.to_string());
            continue;
        }
        if path == "linux-package" || path == "macos" {
            let names: Vec<String> = fields.map(str::to_string).collect();
            if names.is_empty() {
                fail!(
                    "{}: '{path}' takes one or more package names, got '{line}'",
                    file.display()
                );
            }
            if path == "macos" {
                manifest.macos_packages.extend(names);
            } else {
                manifest.linux_packages.extend(names);
            }
            continue;
        }
        if path == "prepare" {
            let (Some(program), None) = (fields.next(), fields.next()) else {
                fail!(
                    "{}: 'prepare' takes one program in the checkout, got '{line}'",
                    file.display()
                );
            };
            if escapes(program) {
                fail!(
                    "{}: '{program}' must be inside the checkout",
                    file.display()
                );
            }
            manifest.prepare.push(program.to_string());
            continue;
        }
        if path == "tool" {
            let (Some(name), Some(program), Some(link), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                fail!(
                    "{}: 'tool' takes a name, a program in the checkout and a link in the checkout, got '{line}'",
                    file.display()
                );
            };
            if !crate::built_tools::is_plain_name(name) {
                fail!(
                    "{}: the tool name '{name}' must be letters, digits, '.', '_' and '-', not starting with '.'",
                    file.display()
                );
            }
            for path in [program, link] {
                if escapes(path) {
                    fail!("{}: '{path}' must be inside the checkout", file.display());
                }
            }
            manifest.tools.push(BuiltTool {
                name: name.to_string(),
                program: program.to_string(),
                link: link.to_string(),
            });
            continue;
        }
        if path == "installed" {
            let (Some(folder), Some(named_in), None) =
                (fields.next(), fields.next(), fields.next())
            else {
                fail!(
                    "{}: 'installed' takes a folder in the cache home and a file in the checkout, got '{line}'",
                    file.display()
                );
            };
            if escapes(folder) {
                fail!(
                    "{}: '{folder}' must be inside the cache home",
                    file.display()
                );
            }
            if escapes(named_in) {
                fail!(
                    "{}: '{named_in}' must be inside the checkout",
                    file.display()
                );
            }
            manifest.installed.push(Installed {
                folder: folder.to_string(),
                file: named_in.to_string(),
            });
            continue;
        }
        let cap = fields.next().unwrap_or("");
        let (mut app_cap, mut lane) = (None, None);
        while let Some(field) = fields.next() {
            match (field, fields.next()) {
                ("app", Some(cap)) if app_cap.is_none() => app_cap = Some(cap),
                ("lane", Some(word)) if lane.is_none() => {
                    match Lane::parse_in(&apps_beside(&file)?, word) {
                        Some(parsed) => lane = Some(parsed),
                        None => fail!(
                            "{}: '{line}' names the lane '{word}', which no .app-build app names",
                            file.display()
                        ),
                    }
                }
                _ => fail!(
                    "{}: '{line}' takes a path, a cap and at most 'app <cap>' and 'lane <lane>'",
                    file.display()
                ),
            }
        }
        if escapes(path) {
            fail!("{}: '{path}' must be inside the checkout", file.display());
        }
        let what = format!("{}: '{path}'", file.display());
        let cap_kib = cap_kib(cap, &what)?;
        let app_cap_kib = app_cap
            .map(|cap| self::cap_kib(cap, &format!("{what} (app)")))
            .transpose()?;
        manifest.dirs.push(BuildDir {
            path: path.to_string(),
            cap_kib,
            app_cap_kib,
            lane,
        });
    }
    Ok(manifest)
}

/// <cap> (20G, 500M) in KiB; fails naming <what> when it is neither.
pub fn cap_kib(cap: &str, what: &str) -> Result<u64> {
    let (digits, unit) = cap.split_at(cap.len().saturating_sub(1));
    if !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n) = digits.parse::<u64>()
    {
        match unit {
            "G" => return Ok(n * 1024 * 1024),
            "M" => return Ok(n * 1024),
            _ => {}
        }
    }
    fail!("{what} needs a cap like 20G or 500M, got '{cap}'")
}

/// A cache every checkout shares: a folder in <root>/caches, with a link to it
/// from where its tool looks, or none when the tool takes the folder's path
/// (Xcode's compilation cache, xcode_cache.rs).
#[derive(Clone, Debug)]
pub struct SharedCache {
    pub name: &'static str,
    pub cap_kib: u64,
    /// Relative to $HOME; None for a cache whose tool is told the folder.
    pub link: Option<&'static str>,
    /// Settings the person writes into the tool's home folder. Builds
    /// copies them into the replacement before it exchanges the folders.
    pub settings: &'static [&'static str],
}

impl SharedCache {
    pub fn link_path(&self, home: &Path) -> Option<PathBuf> {
        self.link.map(|link| home.join(link))
    }
}

const GIB: u64 = 1024 * 1024;

/// The shared caches. Caps are about twice each cache's size when they got
/// them (Oct 2026: gradle 3.4G, npm 0.3G, nuget 2.6G, swiftpm 1.0G). The Xcode compilation
/// cache holds each distinct compile once: 0.6G after one cold build of the
/// bae app's unsigned tree (Nov 2026), so 10G holds many commits' changes. Old
/// Gradle, AGP and NuGet package versions pile up as dependencies move, and
/// deleting a cache costs every checkout a download, so the cap leaves room
/// rather than fitting tightly. sccache's cap is the size `install` writes
/// into its config, and sccache evicts within it itself; its own accounting
/// reads about 0.2 GiB under the folder's size on disk, so status may show
/// it a little over. sccache's cap leaves room for the outputs that agents
/// in every checkout of a day of work compile. When the cache is full,
/// sccache deletes the outputs that compiles read least recently, and agents
/// in other checkouts then compile them again.
///
/// The links for Gradle and NuGet point at their tools' whole home folders,
/// which also hold settings the person writes (gradle.properties, init
/// scripts, NuGet.Config).
pub const SHARED_CACHES: &[SharedCache] = &[
    SharedCache {
        name: "gradle",
        cap_kib: 8 * GIB,
        link: Some(".gradle"),
        settings: &[
            "gradle.properties",
            "init.d",
            "init.gradle",
            "init.gradle.kts",
        ],
    },
    SharedCache {
        name: "npm",
        cap_kib: GIB,
        link: Some(".npm"),
        settings: &[],
    },
    SharedCache {
        name: "nuget",
        cap_kib: 6 * GIB,
        link: Some(".nuget"),
        settings: &["NuGet/NuGet.Config"],
    },
    SharedCache {
        name: "swiftpm",
        cap_kib: 3 * GIB,
        link: Some("Library/Caches/org.swift.swiftpm"),
        settings: &[],
    },
    SharedCache {
        name: "sccache",
        cap_kib: 50 * GIB,
        link: Some("Library/Caches/Mozilla.sccache"),
        settings: &[],
    },
    SharedCache {
        name: crate::xcode_cache::CAS,
        cap_kib: 10 * GIB,
        link: None,
        settings: &[],
    },
];

/// The apps of the `.app-build` beside the manifest <file>, in the same
/// checkout; a checkout with none names only the apps named for platforms.
fn apps_beside(file: &Path) -> Result<boards::app_build::Apps> {
    let config = file.with_file_name(crate::app_config::CONFIG);
    match std::fs::read_to_string(&config) {
        Ok(text) => boards::app_build::Apps::read(&text)
            .map_err(|error| crate::fail::Fail::Message(error.text())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => fail!("reading {}: {error}", config.display()),
    }
}

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
        std::fs::write(
            dir.join(MANIFEST),
            "# a comment\n\ntarget 30G app 12G\n  bae/.build   2G  \nlast 1M",
        )
        .unwrap();
        let entries = build_dirs(&dir).unwrap();
        assert_eq!(
            entries,
            vec![
                BuildDir {
                    path: "target".into(),
                    cap_kib: 30 * GIB,
                    app_cap_kib: Some(12 * GIB),
                    lane: None
                },
                BuildDir {
                    path: "bae/.build".into(),
                    cap_kib: 2 * GIB,
                    app_cap_kib: None,
                    lane: None
                },
                BuildDir {
                    path: "last".into(),
                    cap_kib: 1024,
                    app_cap_kib: None,
                    lane: None
                },
            ]
        );
        assert_eq!(entries[0].cap_kib(false), 30 * GIB);
        assert_eq!(entries[0].cap_kib(true), 12 * GIB);
        assert_eq!(entries[1].cap_kib(true), 2 * GIB);
        for (bad, says) in [
            (
                "target 1G extra\n",
                "takes a path, a cap and at most 'app <cap>' and 'lane <lane>'",
            ),
            (
                "target 1G app\n",
                "takes a path, a cap and at most 'app <cap>' and 'lane <lane>'",
            ),
            (
                "target 1G app 1G more\n",
                "takes a path, a cap and at most 'app <cap>' and 'lane <lane>'",
            ),
            (
                "target 1G app 1X\n",
                "'target' (app) needs a cap like 20G or 500M, got '1X'",
            ),
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
        std::fs::write(
            dir.join(MANIFEST),
            "target 30G app 12G\nxcode 2G app 1G lane mac\ngradle 2G lane android app 1G\n",
        )
        .unwrap();
        let entries = build_dirs(&dir).unwrap();
        let lanes: Vec<Option<Lane>> = entries.iter().map(|entry| entry.lane).collect();
        assert_eq!(lanes, vec![None, Some(Lane::MAC), Some(Lane::ANDROID)]);
        assert_eq!(entries[2].cap_kib(true), GIB);
        // The host folder is every app checkout's; a lane's folder its lane's.
        assert!(entries[0].is_app_in(Some(Lane::MAC)) && entries[0].is_app_in(Some(Lane::ANDROID)));
        assert!(
            entries[1].is_app_in(Some(Lane::MAC)) && !entries[1].is_app_in(Some(Lane::ANDROID))
        );
        assert!(
            !entries[2].is_app_in(Some(Lane::MAC)) && entries[2].is_app_in(Some(Lane::ANDROID))
        );
        assert!(
            !entries[0].is_app_in(None),
            "another checkout has no app folders"
        );
        for bad in [
            "x 1G lane ios\n",
            "x 1G lane\n",
            "x 1G lane mac lane android\n",
            "x 1G app 1G app 2G\n",
        ] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            assert!(build_dirs(&dir).is_err(), "{bad} passed");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn package_lines_name_what_a_linux_host_installs_and_what_needs_a_mac() {
        let dir =
            std::env::temp_dir().join(format!("builds-manifest-packages-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            "target 30G\nlinux-package clang nasm\nlinux-package libasound2-dev\n",
        )
        .unwrap();
        assert_eq!(
            linux_packages(&dir).unwrap(),
            ["clang", "nasm", "libasound2-dev"]
        );
        assert_eq!(build_dirs(&dir).unwrap().len(), 1);
        std::fs::write(
            dir.join(MANIFEST),
            "macos bae-playback\nmacos bae-import bae-util\n",
        )
        .unwrap();
        assert_eq!(
            macos_packages(&dir).unwrap(),
            ["bae-playback", "bae-import", "bae-util"]
        );
        for bad in ["linux-package\n", "macos\n"] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            assert!(linux_packages(&dir).is_err(), "{bad} passed");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keep_lines() {
        let dir = std::env::temp_dir().join(format!("builds-manifest-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), "bae/.build 2G\nkeep bae/.build/run\n").unwrap();
        assert_eq!(
            build_dirs(&dir).unwrap(),
            vec![BuildDir {
                path: "bae/.build".into(),
                cap_kib: 2 * GIB,
                app_cap_kib: None,
                lane: None
            }]
        );
        assert_eq!(
            kept_builds(&dir).unwrap(),
            vec!["bae/.build/run".to_string()]
        );
        for bad in ["keep\n", "keep a b\n", "keep ../x\n"] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            assert!(kept_builds(&dir).is_err(), "{bad} passed");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn installed_lines() {
        let dir =
            std::env::temp_dir().join(format!("builds-manifest-installed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            "target 1G\ninstalled checker scripts/checker.sh\n",
        )
        .unwrap();
        assert_eq!(build_dirs(&dir).unwrap().len(), 1);
        assert_eq!(
            installed(&dir).unwrap(),
            vec![Installed {
                folder: "checker".into(),
                file: "scripts/checker.sh".into(),
            }]
        );
        for (bad, says) in [
            ("installed checker\n", "'installed' takes a folder"),
            ("installed a b c\n", "'installed' takes a folder"),
            ("installed ../x f\n", "must be inside the cache home"),
            ("installed /x f\n", "must be inside the cache home"),
            ("installed x ../f\n", "must be inside the checkout"),
        ] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            match installed(&dir) {
                Err(fail::Fail::Message(message)) => assert!(message.contains(says), "{message}"),
                other => panic!("{bad}: {other:?}"),
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepare_lines() {
        let dir =
            std::env::temp_dir().join(format!("builds-manifest-prepare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            "target 1G\nprepare tools/a.sh\nprepare b.sh\n",
        )
        .unwrap();
        assert_eq!(build_dirs(&dir).unwrap().len(), 1);
        assert_eq!(
            prepare_programs(&dir).unwrap(),
            vec!["tools/a.sh".to_string(), "b.sh".to_string()]
        );
        for (bad, says) in [
            ("prepare\n", "'prepare' takes one program"),
            ("prepare a.sh --fast\n", "'prepare' takes one program"),
            ("prepare ../a.sh\n", "must be inside the checkout"),
            ("prepare /bin/a.sh\n", "must be inside the checkout"),
        ] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            match prepare_programs(&dir) {
                Err(fail::Fail::Message(message)) => assert!(message.contains(says), "{message}"),
                other => panic!("{bad}: {other:?}"),
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tool_lines() {
        let dir = std::env::temp_dir().join(format!("builds-manifest-tool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            "target 1G\ntool lints tools/tool.sh tools/built\n",
        )
        .unwrap();
        assert_eq!(build_dirs(&dir).unwrap().len(), 1);
        assert_eq!(
            built_tools(&dir).unwrap(),
            vec![BuiltTool {
                name: "lints".into(),
                program: "tools/tool.sh".into(),
                link: "tools/built".into(),
            }]
        );
        for (bad, says) in [
            ("tool lints tools/tool.sh\n", "'tool' takes a name"),
            ("tool lints a b c\n", "'tool' takes a name"),
            ("tool .x a b\n", "must be letters"),
            ("tool a/b a b\n", "must be letters"),
            ("tool x ../a b\n", "must be inside the checkout"),
            ("tool x a /b\n", "must be inside the checkout"),
        ] {
            std::fs::write(dir.join(MANIFEST), bad).unwrap();
            match built_tools(&dir) {
                Err(fail::Fail::Message(message)) => assert!(message.contains(says), "{message}"),
                other => panic!("{bad}: {other:?}"),
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
