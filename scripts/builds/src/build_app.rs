//! `builds build-app <repo> mac|android [<ref>]`: builds the person's test app
//! in the repo's app checkout, whose build stays warm, taking turns on its app
//! lock. Nothing is opened. The Mac app is copied out to
//! `~/Applications/<repo>-test`, since the next build there replaces it, in
//! place of the copy there, the running one quit first. The Android build is
//! the repo's own script, which installs the app where it should go.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use crate::app_config::{CONFIG, Config, MacApp, app_checkout_top, check_out, run_command};
use crate::fail;
use crate::fail::{Context, Result};
use crate::lock;
use crate::places::Places;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds build-app <repo> mac|android [<ref>]";
const QUIT_CHECKS: u32 = 60;
const QUIT_CHECK_EVERY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Mac,
    Android,
}

impl Platform {
    fn parse(word: &str) -> Result<Platform> {
        match word {
            "mac" => Ok(Platform::Mac),
            "android" => Ok(Platform::Android),
            _ => fail!("{USAGE}"),
        }
    }

    fn word(self) -> &'static str {
        match self {
            Platform::Mac => "mac",
            Platform::Android => "android",
        }
    }

    fn build(self, config: &Config) -> Result<&[String]> {
        let build = match self {
            Platform::Mac => config.mac.as_ref().map(|mac| &mac.build[..]),
            Platform::Android => config.android.as_ref().map(|android| &android.build[..]),
        };
        match build {
            Some(build) => Ok(build),
            None => fail!("{CONFIG} has no {} lines", self.word()),
        }
    }
}

/// Where the test app lives: outside the checkout, under the home folder.
fn copy_path(places: &Places, repo: &str, mac: &MacApp) -> Result<PathBuf> {
    let Some(name) = Path::new(&mac.app).file_name() else {
        fail!("{CONFIG}: mac app names no file");
    };
    Ok(places.home.join("Applications").join(format!("{repo}-test")).join(name))
}

fn with_new(app: &Path) -> PathBuf {
    let mut path = app.as_os_str().to_owned();
    path.push(".new");
    PathBuf::from(path)
}

/// Copies the built app beside the test app, as `<app>.new`.
fn copy_out(built: &Path, app: &Path) -> Result<()> {
    let folder = app.parent().expect("the test app has a folder");
    std::fs::create_dir_all(folder).context(|| format!("creating {}", folder.display()))?;
    let copy = with_new(app);
    if copy.exists() {
        std::fs::remove_dir_all(&copy).context(|| format!("removing {}", copy.display()))?;
    }
    let status = Command::new("ditto").arg(built).arg(&copy).status().context(|| "running ditto".to_string())?;
    if !status.success() {
        fail!("copying {} to {} failed", built.display(), copy.display());
    }
    Ok(())
}

/// Puts the copy in the test app's place.
fn replace(app: &Path) -> Result<()> {
    if app.exists() {
        std::fs::remove_dir_all(app).context(|| format!("removing {}", app.display()))?;
    }
    let copy = with_new(app);
    std::fs::rename(&copy, app).context(|| format!("moving {} to {}", copy.display(), app.display()))
}

fn running(bundle: &str) -> Result<bool> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(format!("application id \"{bundle}\" is running"))
        .output()
        .context(|| "running osascript".to_string())?;
    if !output.status.success() {
        fail!("asking whether {bundle} runs: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

/// Quits the running app, if one runs, and waits for it to go.
fn quit(bundle: &str, app: &Path) -> Result<()> {
    if !running(bundle)? {
        return Ok(());
    }
    note(format!("quitting the running {bundle}"));
    let status = Command::new("osascript")
        .arg("-e")
        .arg(format!("tell application id \"{bundle}\" to quit"))
        .status()
        .context(|| "running osascript".to_string())?;
    if !status.success() {
        fail!("asking {bundle} to quit failed; the new app is at {}", with_new(app).display());
    }
    for _ in 0..QUIT_CHECKS {
        if !running(bundle)? {
            return Ok(());
        }
        sleep(QUIT_CHECK_EVERY);
    }
    fail!("{bundle} is still running after being asked to quit; the new app is at {}", with_new(app).display())
}

pub fn cmd_build_app(tool: &Tool, args: &[String]) -> Result<()> {
    let (repo, platform, commit) = match args {
        [repo, platform] => (repo, Platform::parse(platform)?, "main"),
        [repo, platform, commit] if !commit.starts_with('-') => (repo, Platform::parse(platform)?, commit.as_str()),
        _ => fail!("{USAGE}"),
    };
    let places = &tool.places;
    let top = app_checkout_top(places, repo)?;

    note(format!("waiting for {repo}'s app lock, then building {commit}'s {} app", platform.word()));
    let mut held = lock::take(places, repo)?;
    // A takeover stops the build; it runs again from the checkout, behind the
    // build that took over.
    let config = loop {
        let config = check_out(&top, commit)?;
        match run_command(&top, platform.build(&config)?)? {
            lock::Ran::Finished => break config,
            lock::Ran::TakenOver => {
                note(format!("the build was stopped for a takeover; building {commit} again after it"));
                held = held.requeue()?;
            }
        }
    };
    let Some(mac) = config.mac.as_ref().filter(|_| platform == Platform::Mac) else {
        held.release();
        return Ok(());
    };
    let app = copy_path(places, repo, mac)?;
    copy_out(&top.join(&mac.app), &app)?;
    // The build and the copy are done: the next build can start.
    held.release();

    quit(&mac.bundle, &app)?;
    replace(&app)?;
    note(format!("built {}", app.display()));
    Ok(())
}

#[cfg(test)]
#[path = "build_app_tests.rs"]
mod tests;
