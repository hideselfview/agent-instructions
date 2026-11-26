//! What each platform of an app needs from the host that builds it, keyed by
//! the shots platform (`boards::app_build::Platform`): the environment its
//! shots are drawn in, whether it builds with Xcode, whether its shots open
//! windows on the host's display, and what `builds build-app` does with the
//! built app. A lane (lane.rs) asks its platform; no other code matches on
//! the lane for these.
//!
//! The environment is the host software that draws the shots. A shot's
//! picture and element list depend on it: the same app draws
//! "largecircle.fill.circle" on one macOS and "Large Filled Circle Inside A
//! Circle" on another, as the accessibility label of one SF Symbol. So builds
//! keys each cached run and each cached shot by the environment it was made in
//! (shots_cache.rs), and a shots run compares only with a run of its own
//! environment.
//!
//! The mac platform's environment is the macOS build and the Xcode build of
//! the host that draws the shots, `<macOS build>-<Xcode build>`
//! (`26B131-17C529`), from `sw_vers -buildVersion` and the `Build version`
//! line of `xcodebuild -version`. The android and web platforms' is the host's
//! OS name and machine architecture from `uname -sm` (`Darwin-arm64`), since
//! Robolectric loads native graphics per OS and architecture. A lane reads it
//! from the host of the checkout it builds in, when it takes the checkout,
//! and never from a constant: a host's Xcode and macOS change when the person
//! upgrades them.

use crate::fail;
use crate::fail::Result;
use boards::app_build::Platform;

/// The command that prints the macOS build of a Mac.
const SW_VERS: &str = "sw_vers -buildVersion";
/// The command that prints the Xcode version and build of a Mac.
const XCODEBUILD: &str = "xcodebuild -version";
/// The command that prints a host's OS name and machine architecture.
const UNAME: &str = "uname -sm";

/// What `builds build-app` does with the app a platform builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    /// Copies the built `.app` into `~/Applications/<repo>-test`, replacing
    /// the running copy.
    CopiedApp,
    /// The build installs the app on the device of the Mac it runs on.
    OnDevice,
}

/// The facts of one platform's row.
pub trait Host {
    /// The environment of the platform's shots on a host that answers the
    /// scripts <run> is given with what they print, or fails with what they
    /// printed when one fails or prints no build.
    fn environment(&self, run: &mut dyn FnMut(&str, &str) -> Result<String>) -> Result<String>;

    /// Whether the platform builds with Xcode, so a host whose Xcode cannot
    /// start (xcode.rs), or whose UI tests time out turning on automation
    /// mode (ui_test_settings.rs), cannot build it.
    fn builds_with_xcode(&self) -> bool;

    /// Whether the platform's steps open windows on the host: they take the
    /// window lock, wait for an unlocked screen and a lid that lets the shot
    /// display draw (host_screen_lock.rs), and keep the display on
    /// (app_remote.rs).
    fn opens_windows(&self) -> bool;

    /// What `builds build-app` does with the built app, or none when the
    /// platform has no `build-app`.
    fn install(&self) -> Option<Install>;
}

struct Mac;
struct Android;
struct Web;

impl Host for Mac {
    fn environment(&self, run: &mut dyn FnMut(&str, &str) -> Result<String>) -> Result<String> {
        let macos = run(SW_VERS, "reading the host's macOS build")?;
        let xcode = run(XCODEBUILD, "reading the host's Xcode build")?;
        parse_mac(&macos, &xcode)
    }

    fn builds_with_xcode(&self) -> bool {
        true
    }

    fn opens_windows(&self) -> bool {
        true
    }

    fn install(&self) -> Option<Install> {
        Some(Install::CopiedApp)
    }
}

impl Host for Android {
    fn environment(&self, run: &mut dyn FnMut(&str, &str) -> Result<String>) -> Result<String> {
        parse_uname(&run(UNAME, "reading the host's OS and architecture")?)
    }

    fn builds_with_xcode(&self) -> bool {
        false
    }

    fn opens_windows(&self) -> bool {
        false
    }

    fn install(&self) -> Option<Install> {
        Some(Install::OnDevice)
    }
}

impl Host for Web {
    fn environment(&self, run: &mut dyn FnMut(&str, &str) -> Result<String>) -> Result<String> {
        parse_uname(&run(UNAME, "reading the host's OS and architecture")?)
    }

    fn builds_with_xcode(&self) -> bool {
        false
    }

    fn opens_windows(&self) -> bool {
        false
    }

    fn install(&self) -> Option<Install> {
        None
    }
}

/// The row of <platform>.
pub fn of(platform: Platform) -> &'static dyn Host {
    match platform {
        Platform::Mac => &Mac,
        Platform::Android => &Android,
        Platform::Web => &Web,
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
pub fn parse_uname(uname: &str) -> Result<String> {
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
#[path = "app_platform_tests.rs"]
mod tests;
