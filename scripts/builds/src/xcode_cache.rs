//! Xcode's compilation cache, shared by every checkout of a host.
//!
//! With `COMPILATION_CACHE_ENABLE_CACHING`, Xcode keys each Swift and Clang
//! compile by its content and stores the result in a content-addressed store.
//! The store's default place is inside each build tree, so a new checkout, whose
//! trees are new, finds none of another checkout's compiles; with
//! `COMPILATION_CACHE_CAS_PATH` naming one folder per host, a compile that any
//! checkout of the host made is a hit for every other checkout. Xcode reads a
//! file named by `XCODE_XCCONFIG_FILE` as a layer of build settings for every
//! xcodebuild it starts, so the app builds set that variable for the steps they
//! run (app_build.rs on this Mac, `builds programs hold` on a host) and no repo
//! script passes the settings. The shared cache `xcode-cas` (manifest.rs) caps
//! the store, which `builds prune` deletes whole when it is over the cap.

use std::path::{Path, PathBuf};

use crate::fail::{Context, Result};
use crate::places::Places;

/// The environment variable Xcode reads a settings file from.
pub const ENV: &str = "XCODE_XCCONFIG_FILE";

/// The shared cache's folder name in <root>/caches.
pub const CAS: &str = "xcode-cas";

/// The settings file, in the build volume's root.
pub fn config_file(places: &Places) -> PathBuf {
    places.root.join(".xcode-compilation-cache.xcconfig")
}

/// The store's folder.
pub fn cas_dir(places: &Places) -> PathBuf {
    places.caches().join(CAS)
}

/// The settings file's text: caching on, with the host's store.
pub fn config_text(cas: &Path) -> String {
    format!(
        "COMPILATION_CACHE_ENABLE_CACHING = YES\nCOMPILATION_CACHE_CAS_PATH = {}\n",
        cas.display()
    )
}

/// Makes the store's folder and the settings file when they differ from what
/// they should hold, and returns the file's path, for `ENV`.
pub fn ensure(places: &Places) -> Result<PathBuf> {
    let cas = cas_dir(places);
    std::fs::create_dir_all(&cas).context(|| format!("creating {}", cas.display()))?;
    let file = config_file(places);
    let text = config_text(&cas);
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
        crate::whole_file::write(&file, text)?;
    }
    Ok(file)
}

#[cfg(test)]
#[path = "xcode_cache_tests.rs"]
mod tests;
