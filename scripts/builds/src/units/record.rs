//! Recording which build copies a build used, from cargo's own report.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    OUTPUT_DIRS, has_shared_folders, hashed_file, is_folder, is_hash, is_profile_dir,
    names_if_present, names_in, stat, unit_dir, unit_folders,
};
use crate::fail;
use crate::fail::{Result, gone};
use crate::text::note;

/// Finds units' fingerprint folders in either layout. For shared folders it
/// keeps each profile folder's .fingerprint listing, by unit hash; a build
/// adds units while it reports them, so a hash not yet listed lists again.
#[derive(Default)]
struct Fingerprints {
    listed: HashMap<PathBuf, HashMap<String, String>>,
}

impl Fingerprints {
    fn list(&mut self, profile: &Path) -> Result<&HashMap<String, String>> {
        let mut units = HashMap::new();
        if has_shared_folders(profile)? {
            for name in names_in(&profile.join(".fingerprint"))? {
                if let Some((_, hash)) = unit_dir(&name) {
                    units.insert(hash.to_string(), name.clone());
                }
            }
        }
        self.listed.insert(profile.to_path_buf(), units);
        Ok(&self.listed[profile])
    }

    fn get(&mut self, profile: &Path, hash: &str) -> Result<Option<PathBuf>> {
        let known = self
            .listed
            .get(profile)
            .is_some_and(|units| units.contains_key(hash));
        let units = if known {
            &self.listed[profile]
        } else {
            self.list(profile)?
        };
        Ok(units
            .get(hash)
            .map(|name| profile.join(".fingerprint").join(name)))
    }

    /// The fingerprint folder of the unit <named> names, when it has one.
    fn of(&mut self, named: &Named) -> Result<Option<PathBuf>> {
        match named {
            Named::Shared(profile, hash) => self.get(profile, hash),
            Named::Own(fingerprint) => Ok(is_folder(fingerprint)?.then(|| fingerprint.clone())),
        }
    }

    /// The fingerprint folders of <package>'s units in <profile>, in either
    /// layout.
    fn by_package(&mut self, profile: &Path, package: &str) -> Result<Vec<PathBuf>> {
        let listed = self
            .listed
            .get(profile)
            .is_some_and(|units| !units.is_empty());
        let units = if listed {
            &self.listed[profile]
        } else {
            self.list(profile)?
        };
        let prefix = format!("{package}-");
        let mut found: Vec<PathBuf> = units
            .values()
            .filter(|name| {
                name.starts_with(&prefix) && unit_dir(name).map(|(p, _)| p) == Some(package)
            })
            .map(|name| profile.join(".fingerprint").join(name))
            .collect();
        for (_, unit) in unit_folders(&profile.join("build").join(package))? {
            found.push(unit.join("fingerprint"));
        }
        Ok(found)
    }
}

fn base(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

fn dirname(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new(""))
}

/// The unit a path cargo reported names, by where the path is.
#[derive(Debug, PartialEq)]
enum Named {
    /// Shared folders: the profile folder and the unit's hash, which names
    /// its folder in .fingerprint.
    Shared(PathBuf, String),
    /// Unit folders: the unit's fingerprint folder.
    Own(PathBuf),
}

/// The unit a path cargo reported is in or names by its hash: a file in its
/// unit folder (build/<package>/<hash>/), deps/<crate>-<hash>*,
/// examples/<name>-<hash>*, or a file in build/<package>-<hash>/.
fn hashed_unit(path: &Path) -> Result<Option<Named>> {
    let parent = dirname(path);
    if OUTPUT_DIRS.contains(&base(parent))
        && let Some((_, hash)) = hashed_file(base(path))
        && has_shared_folders(dirname(parent))?
    {
        return Ok(Some(Named::Shared(
            dirname(parent).to_path_buf(),
            hash.to_string(),
        )));
    }
    let mut parent = parent;
    while !parent.as_os_str().is_empty() && parent != Path::new("/") {
        let above = dirname(parent);
        if is_hash(base(parent))
            && base(dirname(above)) == "build"
            && is_folder(&parent.join("fingerprint"))?
        {
            return Ok(Some(Named::Own(parent.join("fingerprint"))));
        }
        if base(above) == "build"
            && let Some((_, hash)) = unit_dir(base(parent))
            && has_shared_folders(dirname(above))?
        {
            return Ok(Some(Named::Shared(
                dirname(above).to_path_buf(),
                hash.to_string(),
            )));
        }
        parent = above;
    }
    Ok(None)
}

/// The units whose output cargo copied to <path>, a file it put in a profile
/// folder or its examples folder for the user (target/debug/loc-gen for
/// deps/loc_gen-<hash>, or for build/loc-gen/<hash>/out/loc_gen): the copy
/// keeps the original's size and modification time, which no two builds
/// share. In unit folders, the units are <package>'s.
fn copied_from(path: &Path, package: &str) -> Result<Vec<Named>> {
    let parent = dirname(path);
    let (profile, shared) = if base(parent) == "examples" && is_profile_dir(dirname(parent))? {
        (dirname(parent), parent.to_path_buf())
    } else if is_profile_dir(parent)? {
        (parent, parent.join("deps"))
    } else {
        return Ok(Vec::new());
    };
    let Some(copy) = stat(path)? else {
        return Ok(Vec::new());
    };
    let same = |original: &Path| -> Result<bool> {
        Ok(stat(original)?.is_some_and(|original| {
            original.size() == copy.size()
                && original.mtime() == copy.mtime()
                && original.mtime_nsec() == copy.mtime_nsec()
        }))
    };
    let mut found = Vec::new();
    if has_shared_folders(profile)? {
        for name in names_if_present(&shared)? {
            if let Some((_, hash)) = hashed_file(&name)
                && same(&shared.join(&name))?
            {
                found.push(Named::Shared(profile.to_path_buf(), hash.to_string()));
            }
        }
    }
    for (_, unit) in unit_folders(&profile.join("build").join(package))? {
        let out = unit.join("out");
        for name in names_if_present(&out)? {
            if same(&out.join(name))? {
                found.push(Named::Own(unit.join("fingerprint")));
                break;
            }
        }
    }
    Ok(found)
}

/// The package name in a cargo package id:
/// registry+https://...#serde@1.0.228, path+file:///.../bae-loc#0.1.0 (the
/// folder's name when it is the package's).
fn package_name(package_id: &str) -> String {
    let (location, version) = match package_id.rfind('#') {
        Some(at) => (&package_id[..at], &package_id[at + 1..]),
        None => ("", package_id),
    };
    if let Some((name, _)) = version.split_once('@') {
        return name.to_string();
    }
    let location = location
        .split('?')
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    location.rsplit('/').next().unwrap_or("").to_string()
}

fn target_kind(kind: &str) -> &str {
    match kind {
        "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro" => "lib",
        "bin" => "bin",
        "example" => "example",
        "test" => "integration-test",
        "bench" => "bench",
        "custom-build" => "build-script",
        other => other,
    }
}

/// The name cargo gives a unit's fingerprint file: doc-lib-bae_core,
/// bin-loc-gen, test-lib-bae_core.
fn fingerprint_file(message: &Value, doc: bool) -> String {
    let target = &message["target"];
    let flavor = if doc {
        "doc-"
    } else if message["profile"]["test"].as_bool() == Some(true) {
        "test-"
    } else {
        ""
    };
    let kind = target["kind"][0].as_str().unwrap_or("");
    format!(
        "{flavor}{}-{}",
        target_kind(kind),
        target["name"].as_str().unwrap_or("")
    )
}

/// The fingerprints in a unit's fingerprint folder: each file holding one
/// names it in hex, of the number's little-endian bytes, the form a
/// dependent's JSON lists its dependencies' fingerprints in as numbers. A
/// folder or file that no longer exists names none; any other failure to
/// read one is an error that names it.
pub fn fingerprint_values(fingerprint_dir: &Path) -> Result<HashSet<u64>> {
    let mut values = HashSet::new();
    for name in names_if_present(fingerprint_dir)? {
        if name.ends_with(".json")
            || name.starts_with("dep-")
            || name.starts_with("output-")
            || name == "invoked.timestamp"
        {
            continue;
        }
        let path = fingerprint_dir.join(&name);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if gone(&error) => continue,
            // A file whose bytes are no text holds no fingerprint.
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => continue,
            Err(error) => fail!("reading {}: {error}", path.display()),
        };
        let text = text.trim();
        if text.len() % 2 != 0 || text.len() > 16 {
            continue;
        }
        let mut value: u64 = 0;
        let mut ok = true;
        for (i, pair) in text.as_bytes().chunks(2).enumerate() {
            match std::str::from_utf8(pair)
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                Some(byte) => value |= (byte as u64) << (8 * i),
                None => ok = false,
            }
        }
        if ok {
            values.insert(value);
        }
    }
    Ok(values)
}

/// The JSON cargo keeps beside a unit's fingerprint file; an empty object
/// when the file no longer exists or holds no JSON, as when cargo is still
/// writing it. Any other failure to read it is an error that names it.
pub fn recorded(fingerprint_dir: &Path, file_name: &str) -> Result<Value> {
    let path = fingerprint_dir.join(format!("{file_name}.json"));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if gone(&error) => return Ok(Value::Object(Default::default())),
        Err(error) => fail!("reading {}: {error}", path.display()),
    };
    Ok(serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::Object(Default::default())))
}

/// Does a unit's fingerprint JSON record <features>? It holds them in Rust's
/// debug form (["default", "x"]); a form this cannot read counts as the
/// same.
fn same_features(fingerprint: &Value, features: &Value) -> bool {
    let Some(text) = fingerprint.get("features").and_then(Value::as_str) else {
        return true;
    };
    let Ok(Value::Array(recorded)) = serde_json::from_str::<Value>(text) else {
        return true;
    };
    let Value::Array(features) = features else {
        return true;
    };
    let sorted = |values: &Vec<Value>| -> Option<Vec<String>> {
        let mut strings: Vec<String> = values
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<_>>()?;
        strings.sort();
        Some(strings)
    };
    match (sorted(&recorded), sorted(features)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// The fingerprint folders of the units a report of <path> can be, for an
/// output whose name carries no hash: documentation (<target>/doc/<crate>/...)
/// and a library that is also a cdylib or staticlib
/// (target-macos/debug/libbae_bridge.a, or deps/libbae_bridge.rlib when
/// another unit of the build depends on it). Those are the units of the
/// package with this target, mode and features whose dependencies'
/// fingerprints are all among <used>, the fingerprints of the units this
/// build (<record> resolves them once cargo has finished).
/// When none is, every unit with the target, mode and features: their outputs
/// are the same files whichever built them last, so keeping each one's
/// fingerprint keeps nothing but those small folders.
fn units_by_name(
    message: &Value,
    path: &Path,
    fingerprints: &mut Fingerprints,
    used: &HashSet<u64>,
) -> Result<Vec<PathBuf>> {
    let package = package_name(message["package_id"].as_str().unwrap_or(""));
    let mut parent = dirname(path);
    let mut profiles = Vec::new();
    if is_profile_dir(parent)? {
        profiles.push(parent.to_path_buf());
    } else if OUTPUT_DIRS.contains(&base(parent)) && is_profile_dir(dirname(parent))? {
        // A library another unit of the build depends on stays in deps.
        profiles.push(dirname(parent).to_path_buf());
    }
    let mut doc = false;
    if profiles.is_empty() {
        // <target>[/<triple>]/doc/<crate>/index.html, documented from a
        // profile folder beside that doc folder.
        while !parent.as_os_str().is_empty() && parent != Path::new("/") && profiles.is_empty() {
            if base(parent) == "doc" {
                let root = dirname(parent);
                for name in names_if_present(root)? {
                    if is_profile_dir(&root.join(&name))? {
                        profiles.push(root.join(&name));
                    }
                }
                doc = !profiles.is_empty();
            }
            parent = dirname(parent);
        }
    }
    let name = fingerprint_file(message, doc);
    let features = message
        .get("features")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let mut candidates = Vec::new();
    for profile in &profiles {
        for fingerprint_dir in fingerprints.by_package(profile, &package)? {
            if stat(&fingerprint_dir.join(&name))?.is_none() {
                continue;
            }
            let fingerprint = recorded(&fingerprint_dir, &name)?;
            if same_features(&fingerprint, &features) {
                candidates.push((fingerprint_dir, fingerprint));
            }
        }
    }
    let current: Vec<PathBuf> = candidates
        .iter()
        .filter(|(_, fingerprint)| {
            fingerprint
                .get("deps")
                .and_then(Value::as_array)
                .map(|deps| {
                    deps.iter().all(|dep| match dep {
                        Value::Array(parts) => parts
                            .last()
                            .and_then(Value::as_u64)
                            .is_some_and(|v| used.contains(&v)),
                        _ => false,
                    })
                })
                .unwrap_or(true)
        })
        .map(|(dir, _)| dir.clone())
        .collect();
    Ok(if current.is_empty() {
        candidates.into_iter().map(|(dir, _)| dir).collect()
    } else {
        current
    })
}

/// The fingerprint folders of the units a cargo message reports, and the
/// paths it reports that name none.
///
/// An output whose name carries no hash and no deps file it was copied from
/// is not resolved here: which unit it is depends on the fingerprints of its
/// dependencies, and cargo writes a unit's fingerprint after it reports the
/// unit (a pipelined library is reported when its metadata is ready), so
/// the fingerprints read so far can lack a dependency's. Those paths go to
/// <late>, to resolve once cargo has finished. A compiler-artifact message
/// without a package id names no package to look for those outputs in, so
/// its unresolved paths go to <failures>.
fn units_of(
    message: &Value,
    fingerprints: &mut Fingerprints,
    late: &mut Vec<(Value, String)>,
    failures: &mut Vec<String>,
) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let reason = message["reason"].as_str().unwrap_or("");
    let paths: Vec<String> = match reason {
        "build-script-executed" => vec![message["out_dir"].as_str().unwrap_or("").to_string()],
        "compiler-artifact" => {
            let mut paths: Vec<String> = message["filenames"]
                .as_array()
                .map(|names| {
                    names
                        .iter()
                        .filter_map(|n| n.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(executable) = message["executable"].as_str() {
                paths.push(executable.to_string());
            }
            paths
        }
        _ => return Ok((Vec::new(), Vec::new())),
    };
    let mut found: Vec<PathBuf> = Vec::new();
    let mut missing = Vec::new();
    let mut unnamed = Vec::new();
    for path in &paths {
        let Some(named) = hashed_unit(Path::new(path))? else {
            unnamed.push(path.clone());
            continue;
        };
        match fingerprints.of(&named)? {
            Some(dir) => {
                if !found.contains(&dir) {
                    found.push(dir)
                }
            }
            None => missing.push(path.clone()),
        }
    }
    if found.is_empty() && reason == "compiler-artifact" {
        let Some(package_id) = message["package_id"].as_str() else {
            failures.extend(unnamed.iter().map(|path| {
                format!(
                    "cargo reported {path} in a compiler-artifact message without a package_id, so builds cannot tell which package's build copy it is"
                )
            }));
            found.sort();
            return Ok((found, missing));
        };
        let package = package_name(package_id);
        for path in &unnamed {
            for named in copied_from(Path::new(path), &package)? {
                if let Some(dir) = fingerprints.of(&named)?
                    && !found.contains(&dir)
                {
                    found.push(dir);
                }
            }
        }
        if found.is_empty() {
            late.extend(unnamed.into_iter().map(|path| (message.clone(), path)));
        }
    }
    found.sort();
    Ok((found, missing))
}

/// Sets <path>'s access and modification times to <when>.
fn touch(path: &Path, when: &libc::timeval) -> std::io::Result<()> {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("a path holds no NUL");
    let times = [*when, *when];
    if unsafe { libc::utimes(c.as_ptr(), times.as_ptr()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// The time now.
fn now() -> libc::timeval {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970");
    libc::timeval {
        tv_sec: since.as_secs() as libc::time_t,
        tv_usec: since.subsec_micros() as libc::suseconds_t,
    }
}

/// Marks the fingerprint folders <found> of <message> used at <when>, and
/// notes each of <missing> as a failure.
fn mark(
    message: &Value,
    found: Vec<PathBuf>,
    mut missing: Vec<String>,
    when: &libc::timeval,
    touched: &mut Vec<PathBuf>,
    failures: &mut Vec<String>,
) -> Result<()> {
    for dir in found {
        match touch(&dir, when) {
            Ok(()) => touched.push(dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(dir.display().to_string())
            }
            Err(error) => {
                return Err(crate::fail::Fail::Message(format!(
                    "recording use of {}: {error}",
                    dir.display()
                )));
            }
        }
    }
    for path in missing {
        failures.push(format!(
            "cargo reported {path} ({} of {}), and no build copy of it has a fingerprint folder to record its use in",
            message["reason"].as_str().unwrap_or(""),
            message["package_id"].as_str().unwrap_or("?")
        ));
    }
    Ok(())
}

/// Reads cargo's output from <input>. Each of cargo's own messages marks the
/// units it reports as used, at the moment cargo's first message came, the
/// same for every unit of the build: cargo reports nothing before it holds
/// the profile folder's lock, so one build's units are marked after the
/// previous build's. A prune tells two copies one build used (two versions
/// of a crate) from a copy a later build replaced by it. Every other line (a `cargo run` program's
/// output, a test run's) passes through to <output> unchanged. Returns 1 when
/// a reported unit could not be recorded, else 0. An error while recording
/// stops the recording, but this reads the rest of cargo's output, passing
/// it through, before it returns the error, so cargo never waits on a full
/// pipe.
pub fn record(mut input: impl BufRead, output: &mut impl Write) -> Result<i32> {
    let mut error: Option<crate::fail::Fail> = None;
    let mut fingerprints = Fingerprints::default();
    let mut touched: Vec<PathBuf> = Vec::new();
    let mut late: Vec<(Value, String)> = Vec::new();
    let mut failures = Vec::new();
    let mut output_open = true;
    let mut began: Option<libc::timeval> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = input.read_until(b'\n', &mut line).map_err(|error| {
            crate::fail::Fail::Message(format!("reading cargo's output: {error}"))
        })?;
        if read == 0 {
            break;
        }
        let message: Option<Value> = if line.starts_with(b"{\"reason\":") {
            serde_json::from_slice(&line).ok()
        } else {
            None
        };
        let Some(message) = message else {
            if output_open && (output.write_all(&line).is_err() || output.flush().is_err()) {
                output_open = false;
            }
            continue;
        };
        if error.is_some() {
            continue;
        }
        let when = *began.get_or_insert_with(now);
        let recorded = units_of(&message, &mut fingerprints, &mut late, &mut failures).and_then(
            |(found, missing)| mark(&message, found, missing, &when, &mut touched, &mut failures),
        );
        if let Err(failed) = recorded {
            error = Some(failed);
        }
    }
    if let Some(error) = error {
        return Err(error);
    }
    // Cargo has finished: every unit's fingerprint is written, so the units
    // the late outputs can be are the ones built against the units this build used.
    let mut used: HashSet<u64> = HashSet::new();
    for dir in &touched {
        used.extend(fingerprint_values(dir)?);
    }
    let when = began.unwrap_or_else(now);
    for (message, path) in late {
        let found = units_by_name(&message, Path::new(&path), &mut fingerprints, &used)?;
        let missing = if found.is_empty() {
            vec![path]
        } else {
            Vec::new()
        };
        for dir in &found {
            used.extend(fingerprint_values(dir)?);
        }
        mark(&message, found, missing, &when, &mut touched, &mut failures)?;
    }
    for failure in &failures {
        note(failure);
    }
    Ok(if failures.is_empty() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cargo's output, then, once all of it has been read, the moment cargo
    /// writes the fingerprints of the units it reported.
    struct Then<R, F: FnMut()> {
        input: R,
        at_end: F,
    }

    impl<R: std::io::Read, F: FnMut()> std::io::Read for Then<R, F> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let read = self.input.read(buf)?;
            if read == 0 {
                (self.at_end)();
            }
            Ok(read)
        }
    }

    #[test]
    fn a_library_with_no_hash_in_its_name_marks_only_the_copy_built_against_the_units_the_build_used()
     {
        let scratch = crate::testing::Scratch::new("record-late");
        let profile = scratch.dir.join("debug");
        let fingerprints = profile.join(".fingerprint");
        let (dep, new_app, old_app) = (
            "dep-aaaaaaaaaaaaaaaa",
            "app-bbbbbbbbbbbbbbbb",
            "app-cccccccccccccccc",
        );
        // The fingerprint of the dependency this build made, as little-endian hex.
        let value: u64 = 0x0102_0304_0506_0708;
        let writes = |name: &str, file: &str, dep_value: u64| {
            let dir = fingerprints.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), "0000000000000000").unwrap();
            std::fs::write(
                dir.join(format!("{file}.json")),
                format!("{{\"deps\": [[0, \"dep\", false, {dep_value}]]}}"),
            )
            .unwrap();
        };
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        std::fs::create_dir_all(fingerprints.join(dep)).unwrap();
        writes(new_app, "lib-app", value);
        writes(old_app, "lib-app", value + 1);
        let long_ago = libc::timeval {
            tv_sec: 1_000_000,
            tv_usec: 0,
        };
        for name in [new_app, old_app] {
            touch(&fingerprints.join(name), &long_ago).unwrap();
        }
        let dep_artifact = format!(
            "{{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///ws/dep#dep@0.1.0\",\"target\":{{\"kind\":[\"lib\"],\"name\":\"dep\"}},\"profile\":{{}},\"filenames\":[\"{}/deps/libdep-aaaaaaaaaaaaaaaa.rmeta\"]}}\n",
            profile.display()
        );
        let app_artifact = format!(
            "{{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///ws/app#app@0.1.0\",\"target\":{{\"kind\":[\"lib\",\"staticlib\"],\"name\":\"app\"}},\"profile\":{{}},\"filenames\":[\"{}/libapp.a\"]}}\n",
            profile.display()
        );
        let input = Then {
            input: std::io::Cursor::new(format!("{dep_artifact}{app_artifact}")),
            at_end: || {
                std::fs::write(fingerprints.join(dep).join("lib-dep"), "0807060504030201").unwrap();
            },
        };
        let code = record(std::io::BufReader::new(input), &mut Vec::new()).unwrap();
        assert_eq!(code, 0);
        let mtime = |name: &str| {
            std::fs::metadata(fingerprints.join(name))
                .unwrap()
                .modified()
                .unwrap()
        };
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        assert!(
            mtime(new_app) > old,
            "the copy built against this build's dep was not marked used"
        );
        assert_eq!(
            mtime(old_app),
            old,
            "a copy built against another dep was marked used"
        );
    }

    /// A path in a unit folder names that unit, a path in shared folders
    /// names its unit by hash, and a copy cargo made for the user names the
    /// unit folder it came from, in a profile folder that holds both layouts.
    #[test]
    fn paths_name_their_units_in_either_layout() {
        let scratch = crate::testing::Scratch::new("record-layouts");
        let profile = scratch.dir.join("debug");
        let unit = profile.join("build/loc-gen/0123456789abcdef");
        std::fs::create_dir_all(unit.join("fingerprint")).unwrap();
        std::fs::create_dir_all(unit.join("out")).unwrap();
        std::fs::write(unit.join("out/loc_gen"), "a binary").unwrap();
        std::fs::create_dir_all(profile.join(".fingerprint/dep-fedcba9876543210")).unwrap();
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        let own = Some(Named::Own(unit.join("fingerprint")));
        assert_eq!(hashed_unit(&unit.join("out/loc_gen")).unwrap(), own);
        // A build script's run reports its out folder.
        assert_eq!(hashed_unit(&unit.join("out")).unwrap(), own);
        assert_eq!(
            hashed_unit(&profile.join("deps/libdep-fedcba9876543210.rlib")).unwrap(),
            Some(Named::Shared(
                profile.clone(),
                "fedcba9876543210".to_string()
            ))
        );
        assert_eq!(hashed_unit(&profile.join("loc-gen")).unwrap(), None);
        let copy = profile.join("loc-gen");
        std::fs::write(&copy, "a binary").unwrap();
        let when = libc::timeval {
            tv_sec: 1_000_000,
            tv_usec: 0,
        };
        touch(&unit.join("out/loc_gen"), &when).unwrap();
        touch(&copy, &when).unwrap();
        assert_eq!(
            copied_from(&copy, "loc-gen").unwrap(),
            vec![Named::Own(unit.join("fingerprint"))]
        );
        assert_eq!(copied_from(&copy, "other").unwrap(), Vec::new());
    }

    /// A compiler-artifact message without a package id is reported as a
    /// failure, never looked up under an empty package name, which would
    /// search build/ itself.
    #[test]
    fn an_artifact_without_a_package_id_is_a_failure() {
        let scratch = crate::testing::Scratch::new("record-no-package-id");
        let profile = scratch.dir.join("debug");
        let original = profile.join("build/0123456789abcdef/out/libx.a");
        crate::testing::fill(&profile.join("build/0123456789abcdef/fingerprint/lib-x"), 1);
        crate::testing::fill(&original, 1);
        // A unit folder that makes debug a profile folder.
        crate::testing::fill(
            &profile.join("build/y/fedcba9876543210/fingerprint/lib-y"),
            1,
        );
        let copy = profile.join("libx.a");
        std::fs::copy(&original, &copy).unwrap();
        let when = libc::timeval {
            tv_sec: 1_000_000,
            tv_usec: 0,
        };
        touch(&original, &when).unwrap();
        touch(&copy, &when).unwrap();
        let message = format!(
            "{{\"reason\":\"compiler-artifact\",\"target\":{{\"kind\":[\"staticlib\"],\"name\":\"x\"}},\"profile\":{{}},\"filenames\":[\"{}\"]}}\n",
            copy.display()
        );
        let code = record(std::io::Cursor::new(message), &mut Vec::new()).unwrap();
        assert_eq!(code, 1);
    }

    /// An error while recording fails the recording, after the rest of
    /// cargo's output has passed through.
    #[test]
    fn an_error_while_recording_still_passes_the_rest_of_the_output_through() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = crate::testing::Scratch::new("record-error");
        let unit = scratch.dir.join("debug/build/x/0123456789abcdef");
        crate::testing::fill(&unit.join("fingerprint/lib-x"), 1);
        let message = format!(
            "{{\"reason\":\"build-script-executed\",\"package_id\":\"x\",\"out_dir\":\"{}\"}}\nafter\n",
            unit.join("out").display()
        );
        std::fs::set_permissions(&unit, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut output = Vec::new();
        let result = record(std::io::Cursor::new(message), &mut output);
        std::fs::set_permissions(&unit, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            result.is_err(),
            "a fingerprint folder that cannot be read was recorded"
        );
        assert_eq!(output, b"after\n");
    }

    #[test]
    fn package_names() {
        assert_eq!(
            package_name("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.228"),
            "serde"
        );
        assert_eq!(
            package_name("path+file:///Users/x/bae/bae-loc#0.1.0"),
            "bae-loc"
        );
        assert_eq!(package_name("path+file:///Users/x/ws/dep#dep@0.1.0"), "dep");
        assert_eq!(
            package_name("git+https://github.com/bae-fm/coven?rev=abc#coven@0.1.0"),
            "coven"
        );
    }
}
