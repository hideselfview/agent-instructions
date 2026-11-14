//! Recording which build copies a build used, from cargo's own report.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{OUTPUT_DIRS, hashed_file, is_profile_dir, names_in, unit_dir};
use crate::fail::Result;
use crate::text::note;

/// The fingerprint folders of each profile folder, by unit hash. A build adds
/// units while it reports them, so a hash not yet listed lists again.
#[derive(Default)]
struct Fingerprints {
    listed: HashMap<PathBuf, HashMap<String, String>>,
}

impl Fingerprints {
    fn list(&mut self, profile: &Path) -> Result<&HashMap<String, String>> {
        let mut units = HashMap::new();
        for name in names_in(&profile.join(".fingerprint"))? {
            if let Some((_, hash)) = unit_dir(&name) {
                units.insert(hash.to_string(), name.clone());
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
        Ok(units
            .values()
            .filter(|name| {
                name.starts_with(&prefix) && unit_dir(name).map(|(p, _)| p) == Some(package)
            })
            .map(|name| profile.join(".fingerprint").join(name))
            .collect())
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

/// (<profile folder>, <hash>) of a path cargo reported, when the path names
/// its unit's hash: deps/<crate>-<hash>*, examples/<name>-<hash>*, or a file
/// in build/<package>-<hash>/.
fn hashed_unit(path: &Path) -> Option<(PathBuf, String)> {
    let parent = dirname(path);
    if OUTPUT_DIRS.contains(&base(parent))
        && let Some((_, hash)) = hashed_file(base(path))
        && is_profile_dir(dirname(parent))
    {
        return Some((dirname(parent).to_path_buf(), hash.to_string()));
    }
    let mut parent = parent;
    while !parent.as_os_str().is_empty() && parent != Path::new("/") {
        let above = dirname(parent);
        if base(above) == "build"
            && let Some((_, hash)) = unit_dir(base(parent))
            && is_profile_dir(dirname(above))
        {
            return Some((dirname(above).to_path_buf(), hash.to_string()));
        }
        parent = above;
    }
    None
}

/// The (<profile folder>, <hash>) of each unit whose deps (or examples) file
/// cargo copied to <path>, a target cargo put beside them
/// (target/debug/loc-gen for deps/loc_gen-<hash>): the copy keeps the
/// original's size and modification time, which no two builds share.
fn copied_from(path: &Path) -> Vec<(PathBuf, String)> {
    let parent = dirname(path);
    let (profile, outputs) = if is_profile_dir(parent) {
        (parent.to_path_buf(), parent.join("deps"))
    } else if base(parent) == "examples" && is_profile_dir(dirname(parent)) {
        (dirname(parent).to_path_buf(), parent.to_path_buf())
    } else {
        return Vec::new();
    };
    let (Ok(copy), Ok(names)) = (std::fs::metadata(path), names_in(&outputs)) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for name in names {
        let Some((_, hash)) = hashed_file(&name) else {
            continue;
        };
        let Ok(original) = std::fs::metadata(outputs.join(&name)) else {
            continue;
        };
        if original.size() == copy.size()
            && original.mtime() == copy.mtime()
            && original.mtime_nsec() == copy.mtime_nsec()
        {
            found.push((profile.clone(), hash.to_string()));
        }
    }
    found
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
/// dependent's JSON lists its dependencies' fingerprints in as numbers.
pub fn fingerprint_values(fingerprint_dir: &Path) -> HashSet<u64> {
    let mut values = HashSet::new();
    let Ok(names) = names_in(fingerprint_dir) else {
        return values;
    };
    for name in names {
        if name.ends_with(".json")
            || name.starts_with("dep-")
            || name.starts_with("output-")
            || name == "invoked.timestamp"
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(fingerprint_dir.join(&name)) else {
            continue;
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
    values
}

/// The JSON cargo keeps beside a unit's fingerprint file; an empty object
/// when unreadable.
pub fn recorded(fingerprint_dir: &Path, file_name: &str) -> Value {
    std::fs::read(fingerprint_dir.join(format!("{file_name}.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| Value::Object(Default::default()))
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
/// build reported before it (cargo reports a unit after its dependencies).
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
    if is_profile_dir(parent) {
        profiles.push(parent.to_path_buf());
    } else if OUTPUT_DIRS.contains(&base(parent)) && is_profile_dir(dirname(parent)) {
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
                for name in names_in(root).unwrap_or_default() {
                    if is_profile_dir(&root.join(&name)) {
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
            if !fingerprint_dir.join(&name).exists() {
                continue;
            }
            let fingerprint = recorded(&fingerprint_dir, &name);
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
fn units_of(
    message: &Value,
    fingerprints: &mut Fingerprints,
    used: &HashSet<u64>,
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
        let Some((profile, hash)) = hashed_unit(Path::new(path)) else {
            unnamed.push(path.clone());
            continue;
        };
        match fingerprints.get(&profile, &hash)? {
            Some(dir) => {
                if !found.contains(&dir) {
                    found.push(dir)
                }
            }
            None => missing.push(path.clone()),
        }
    }
    if found.is_empty() && reason == "compiler-artifact" {
        for path in &unnamed {
            for (profile, hash) in copied_from(Path::new(path)) {
                if let Some(dir) = fingerprints.get(&profile, &hash)?
                    && !found.contains(&dir)
                {
                    found.push(dir);
                }
            }
        }
        if found.is_empty() {
            for path in &unnamed {
                for dir in units_by_name(message, Path::new(path), fingerprints, used)? {
                    if !found.contains(&dir) {
                        found.push(dir);
                    }
                }
            }
        }
        if found.is_empty() {
            missing.extend(unnamed);
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

/// Reads cargo's output from <input>. Each of cargo's own messages marks the
/// units it reports as used, at the moment cargo's first message came, the
/// same for every unit of the build: cargo reports nothing before it holds
/// the profile folder's lock, so one build's units are marked after the
/// previous build's. A prune tells two copies one build used (two versions
/// of a crate) from a copy a later build replaced by it. Every other line (a `cargo run` program's
/// output, a test run's) passes through to <output> unchanged. Returns 1 when
/// a reported unit could not be recorded, else 0.
pub fn record(mut input: impl BufRead, output: &mut impl Write) -> Result<i32> {
    let mut fingerprints = Fingerprints::default();
    let mut used: HashSet<u64> = HashSet::new();
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
        let when = *began.get_or_insert_with(now);
        let (found, mut missing) = units_of(&message, &mut fingerprints, &used)?;
        for dir in found {
            match touch(&dir, &when) {
                Ok(()) => used.extend(fingerprint_values(&dir)),
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
    }
    for failure in &failures {
        note(failure);
    }
    Ok(if failures.is_empty() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

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
