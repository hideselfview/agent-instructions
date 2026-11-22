//! The environment a shots run is made in: the host software that draws the
//! shots. A shot's picture and element list depend on it: the same app draws
//! "largecircle.fill.circle" on one macOS and "Large Filled Circle Inside A
//! Circle" on another, as the accessibility label of one SF Symbol. So builds
//! keys each cached run and each cached shot by the environment it was made in
//! (shots_cache.rs), and a shots run compares only with a run of its own
//! environment.
//!
//! The mac lane's environment is the macOS build and the Xcode build of the
//! host that draws the shots, `<macOS build>-<Xcode build>` (`26B131-17C529`),
//! from `sw_vers -buildVersion` and the `Build version` line of `xcodebuild
//! -version`. The android lane's is the host's OS name and machine
//! architecture from `uname -sm` (`Darwin-arm64`), since Robolectric loads
//! native graphics per OS and architecture. A lane reads it from the host of
//! the checkout it builds in, when it takes the checkout, and never from a
//! constant: a host's Xcode and macOS change when the person upgrades them.

use crate::fail;
use crate::fail::Result;
use crate::lane::Lane;

/// The command that prints the macOS build of a Mac.
const SW_VERS: &str = "sw_vers -buildVersion";
/// The command that prints the Xcode version and build of a Mac.
const XCODEBUILD: &str = "xcodebuild -version";
/// The command that prints a host's OS name and machine architecture.
const UNAME: &str = "uname -sm";

/// The environment of <lane> on a host that answers the scripts <run> is
/// given with what they print, or fails with what they printed when one
/// fails or prints no build.
pub fn read(lane: Lane, run: &mut dyn FnMut(&str, &str) -> Result<String>) -> Result<String> {
    match lane {
        Lane::Mac => {
            let macos = run(SW_VERS, "reading the host's macOS build")?;
            let xcode = run(XCODEBUILD, "reading the host's Xcode build")?;
            parse_mac(&macos, &xcode)
        }
        Lane::Android => parse_android(&run(UNAME, "reading the host's OS and architecture")?),
    }
}

/// The mac environment that <sw_vers> (what `sw_vers -buildVersion` printed)
/// and <xcodebuild> (what `xcodebuild -version` printed) name.
pub fn parse_mac(sw_vers: &str, xcodebuild: &str) -> Result<String> {
    let macos = sw_vers.trim();
    if macos.is_empty() || macos.contains(char::is_whitespace) {
        fail!("`{SW_VERS}` printed {sw_vers:?}, which is no macOS build");
    }
    let Some(xcode) = xcodebuild
        .lines()
        .find_map(|line| line.trim().strip_prefix("Build version "))
        .map(str::trim)
        .filter(|build| !build.is_empty() && !build.contains(char::is_whitespace))
    else {
        fail!("`{XCODEBUILD}` printed no `Build version` line: {xcodebuild:?}");
    };
    folder_safe(&format!("{macos}-{xcode}"))
}

/// The android environment that <uname> (what `uname -sm` printed) names:
/// the OS name and the machine, joined by a dash.
pub fn parse_android(uname: &str) -> Result<String> {
    let words: Vec<&str> = uname.split_whitespace().collect();
    let [system, machine] = words[..] else {
        fail!("`{UNAME}` printed {uname:?}, which is no OS name and machine");
    };
    folder_safe(&format!("{system}-{machine}"))
}

/// <environment> when it can be one folder's name in the shots cache.
fn folder_safe(environment: &str) -> Result<String> {
    if !environment
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        fail!("{environment:?} cannot be a folder's name in the shots cache");
    }
    Ok(environment.to_string())
}

#[cfg(test)]
#[path = "shots_environment_tests.rs"]
mod tests;
