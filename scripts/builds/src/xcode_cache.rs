//! Xcode's compilation cache, shared by every checkout of a host.
//!
//! With `COMPILATION_CACHE_ENABLE_CACHING`, Xcode keys each Swift and Clang
//! compile by its content and stores the result in a content-addressed store.
//! The store's default place is inside each build tree, so a new checkout, whose
//! trees are new, finds none of another checkout's compiles; with
//! `COMPILATION_CACHE_CAS_PATH` naming one folder per host, a checkout finds
//! the compiles of any checkout whose command lines match its own. Those hold
//! the checkout's and the build tree's absolute paths, so with a new enough
//! Xcode the settings file also maps those folders to fixed names, and the
//! command lines, and with them the keys, match across checkouts. Xcode reads a
//! file named by `XCODE_XCCONFIG_FILE` as a layer of build settings for every
//! xcodebuild it starts, so the app builds set that variable for the steps they
//! run (app_build.rs on this Mac, `builds programs hold` on a host) and no repo
//! script passes the settings. The shared cache `xcode-cas` (manifest.rs) caps
//! the store, which `builds prune` deletes whole when it is over the cap.

use std::path::{Path, PathBuf};

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;

/// The environment variable Xcode reads a settings file from.
pub const ENV: &str = "XCODE_XCCONFIG_FILE";

/// The shared cache's folder name in <root>/caches.
pub const CAS: &str = "xcode-cas";

/// The folder in the build volume's root that holds each checkout's settings
/// file. Its name starts with a dot, so it is none of the volume's repos
/// (`Places::is_reserved_name`).
const SETTINGS: &str = ".xcode-settings";

/// The checkout's settings file, in the build volume's root.
pub fn config_file(places: &Places, checkout: &Checkout) -> PathBuf {
    places
        .root
        .join(SETTINGS)
        .join(checkout.repo())
        .join(format!("{}.xcconfig", checkout.name()))
}

/// The store's folder.
pub fn cas_dir(places: &Places) -> PathBuf {
    places.caches().join(CAS)
}

/// The first Xcode that builds bae with path mapping. Xcode 26.3's
/// swift-frontend crashes in bae's `bae` target when the settings map paths
/// (bad_optional_access in CachingDiagnosticsProcessor, exit 65); Xcode 27.2
/// builds it, and a second checkout of the same commit then hits every compile
/// (measured 2026-11-21).
const MAPPING_FROM: (u32, u32) = (27, 2);

/// The version in the first line `xcodebuild -version` prints ("Xcode 27.2"),
/// None for any other text.
fn parse_xcode_version(text: &str) -> Option<(u32, u32)> {
    let words = text.lines().next()?.strip_prefix("Xcode ")?;
    let mut parts = words.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor))
}

/// Whether the Xcode on this machine builds with path mapping: false when
/// the machine has no xcodebuild or its version is older than `MAPPING_FROM`;
/// an error when xcodebuild fails or prints what the function cannot read,
/// since such an Xcode would build without mapping and nothing would say why.
pub fn xcode_maps_paths() -> Result<bool> {
    let out = match std::process::Command::new("xcodebuild")
        .arg("-version")
        .output()
    {
        Ok(out) => out,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => fail!("running xcodebuild -version: {error}"),
    };
    let said = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        fail!(
            "xcodebuild -version failed ({}): {}{}",
            out.status,
            said.trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    match parse_xcode_version(&said) {
        Some(version) => Ok(version >= MAPPING_FROM),
        None => fail!(
            "xcodebuild -version printed no Xcode version: {}",
            said.trim()
        ),
    }
}

/// The settings file for the checkout that <folder> is in, made for this
/// machine's Xcode (`ensure`), for `ENV`. Fails when <folder> is in no git
/// checkout.
pub fn settings_for(places: &Places, folder: &Path) -> Result<PathBuf> {
    let Some(checkout) = Checkout::at(folder)? else {
        fail!("{} is in no git checkout", folder.display());
    };
    ensure(places, &checkout, xcode_maps_paths()?)
}

/// The settings file's text: caching on, with the host's store. With
/// <maps> it also maps the checkout's own folders (its working tree <top> and
/// its folder <build> in the build volume) to the same names in every
/// checkout, so that the compile commands, and with them the cache's keys,
/// are the same in each. Xcode takes a mapping as `from=to` words, so a path
/// with a space cannot be one.
pub fn config_text(cas: &Path, top: &Path, build: &Path, maps: bool) -> Result<String> {
    let mut text = format!(
        "COMPILATION_CACHE_ENABLE_CACHING = YES\nCOMPILATION_CACHE_CAS_PATH = {}\n",
        cas.display()
    );
    if !maps {
        return Ok(text);
    }
    for path in [top, build] {
        if path.to_string_lossy().contains(char::is_whitespace) {
            fail!(
                "{} has a space, which Xcode's path mapping cannot take",
                path.display()
            );
        }
    }
    let maps = format!("{}=/^checkout {}=/^build", top.display(), build.display());
    text.push_str(&format!(
        "SWIFT_ENABLE_PREFIX_MAPPING = YES\nCLANG_ENABLE_PREFIX_MAPPING = YES\nSWIFT_OTHER_PREFIX_MAPPINGS = {maps}\nCLANG_OTHER_PREFIX_MAPPINGS = {maps}\n"
    ));
    Ok(text)
}

/// Makes the store's folder and the checkout's settings file when they differ
/// from what they should hold, and returns the file's path, for `ENV`.
/// <maps> says whether this machine's Xcode builds with path mapping
/// (`xcode_maps_paths`).
pub fn ensure(places: &Places, checkout: &Checkout, maps: bool) -> Result<PathBuf> {
    let cas = cas_dir(places);
    std::fs::create_dir_all(&cas).context(|| format!("creating {}", cas.display()))?;
    let file = config_file(places, checkout);
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder).context(|| format!("creating {}", folder.display()))?;
    }
    let text = config_text(&cas, &checkout.top, &checkout.build_root(places), maps)?;
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
        crate::whole_file::write(&file, text)?;
    }
    Ok(file)
}

#[cfg(test)]
#[path = "xcode_cache_tests.rs"]
mod tests;
