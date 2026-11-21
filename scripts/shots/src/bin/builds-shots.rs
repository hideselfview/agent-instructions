//! builds-shots: the shots checker (lib.rs) as a program of its own, with
//! the version its build script computes (build.rs) and the cache folder
//! `BUILDS_SHOTS_CACHE` names, which builds sets to a folder on its build
//! volume, else `~/.cache/builds-shots`.

use std::path::PathBuf;
use std::process::ExitCode;

/// The version of this program: a hash of every file cargo builds it from
/// (build.rs).
const VERSION: &str = env!("SHOTS_SOURCE");

/// The program's cache folder.
fn cache_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("BUILDS_SHOTS_CACHE") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "neither BUILDS_SHOTS_CACHE nor HOME is set".to_string())?;
    Ok(PathBuf::from(home).join(".cache/builds-shots"))
}

fn main() -> ExitCode {
    let cache = match cache_dir() {
        Ok(cache) => cache,
        Err(message) => {
            eprintln!("builds-shots: {message}");
            return ExitCode::FAILURE;
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let checker = shots::Checker {
        version: VERSION,
        cache: &cache,
    };
    if shots::main(&checker, &args) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
