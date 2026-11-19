//! `builds build-app <repo> mac|android [<ref>]`: builds the person's test app
//! in an app checkout of the platform's lane (lane.rs), whose build stays
//! warm, taking turns on that lane's app lock. Nothing is opened. The Mac
//! build takes a checkout on this Mac or on a host, as `builds app-build`
//! does, except when its command is a step that a `here` line names, which
//! takes only a checkout on this Mac, and a build on a host copies the built app back here. The Mac app is
//! copied out to `~/Applications/<repo>-test`, since the next build there
//! replaces it, in place of the copy there, the running one quit first. The
//! Android build is the repo's own script, which installs the app where it
//! should go, so it takes only a checkout on this Mac.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use crate::app_build::config_at;
use crate::app_config::{CONFIG, Config, MacApp, run_command};
use crate::app_site::Site;
use crate::fail;
use crate::fail::{Context, Result};
use crate::host::Ssh;
use crate::lane::Lane;
use crate::lock::{self, Where, Within};
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

    fn lane(self) -> Lane {
        match self {
            Platform::Mac => Lane::Mac,
            Platform::Android => Lane::Android,
        }
    }

    /// Which of the lane's checkouts the build may take: the Mac build, on
    /// any host's too, since the build copies its app back here, unless its
    /// command is one a `here` line names (a step only this Mac can run, such
    /// as the signed app); the Android build installs the app on the device
    /// of the Mac it runs on.
    fn allowed(self, config: &Config) -> Where {
        match self {
            Platform::Mac => {
                let command = config.mac.as_ref().map(|mac| &mac.build);
                let here = config
                    .steps
                    .iter()
                    .any(|step| step.place == Where::Here && Some(&step.command) == command);
                if here { Where::Here } else { Where::Anywhere }
            }
            Platform::Android => Where::Here,
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
    Ok(places
        .home
        .join("Applications")
        .join(format!("{repo}-test"))
        .join(name))
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
    let status = Command::new("ditto")
        .arg(built)
        .arg(&copy)
        .status()
        .context(|| "running ditto".to_string())?;
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
    std::fs::rename(&copy, app)
        .context(|| format!("moving {} to {}", copy.display(), app.display()))
}

fn running(bundle: &str) -> Result<bool> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(format!("application id \"{bundle}\" is running"))
        .output()
        .context(|| "running osascript".to_string())?;
    if !output.status.success() {
        fail!(
            "asking whether {bundle} runs: {}",
            String::from_utf8_lossy(&output.stderr)
        );
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
        fail!(
            "asking {bundle} to quit failed; the new app is at {}",
            with_new(app).display()
        );
    }
    for _ in 0..QUIT_CHECKS {
        if !running(bundle)? {
            return Ok(());
        }
        sleep(QUIT_CHECK_EVERY);
    }
    fail!(
        "{bundle} is still running after being asked to quit; the new app is at {}",
        with_new(app).display()
    )
}

/// What a build with no ref builds: origin's main as the fetch left it. The
/// app checkout's own `main` can lag behind it.
const DEFAULT_REF: &str = "origin/main";

fn parse_args(args: &[String]) -> Result<(&String, Platform, &str)> {
    match args {
        [repo, platform] => Ok((repo, Platform::parse(platform)?, DEFAULT_REF)),
        [repo, platform, commit] if !commit.starts_with('-') => {
            Ok((repo, Platform::parse(platform)?, commit.as_str()))
        }
        _ => fail!("{USAGE}"),
    }
}

/// Runs the build <command> in <site>, with no time limit: here, or on the
/// host through `builds remote step`.
fn run_build(site: &mut Site, command: &[String]) -> Result<Within> {
    match site {
        Site::Local(top) => run_command(top, command, None),
        Site::Remote(remote) => {
            lock::run_in_group_within(remote.step_command(command)?, &command[0], None)
        }
    }
}

/// The folder on this Mac that holds the built app of <mac>: the checkout
/// itself, or for a checkout on a host the folder the build's output was
/// copied to.
fn built_app(site: &mut Site, mac: &MacApp) -> Result<PathBuf> {
    match site {
        Site::Local(top) => Ok(top.join(&mac.app)),
        Site::Remote(remote) => {
            remote.pull_bundle(&mac.app)?;
            Ok(remote.local_folder().join(&mac.app))
        }
    }
}

pub fn cmd_build_app(tool: &Tool, args: &[String]) -> Result<()> {
    let (repo, platform, commit) = parse_args(args)?;
    let places = &tool.places;
    let ssh = Ssh::from_env();

    note(format!(
        "waiting for {repo}'s {} app lock, then building {commit}'s {} app",
        platform.word(),
        platform.word()
    ));
    let allowed = platform.allowed(&config_at(places, repo, commit)?);
    let mut held = lock::take_where(
        places,
        &ssh,
        repo,
        platform.lane(),
        allowed,
        tool.rules.floor_kib,
    )?;
    // A takeover stops the build; it runs again from a checkout, behind the
    // build that took over.
    let (config, mut site) = loop {
        let mut site = Site::of(places, &ssh, repo, platform.lane(), &held.checkout)?;
        note(format!("building in {}", held.checkout));
        let config = site.check_out(places, repo, commit)?.config;
        match run_build(&mut site, platform.build(&config)?)? {
            Within::Ran(lock::Ran::Finished) => break (config, site),
            Within::Ran(lock::Ran::TakenOver) => {
                note(format!(
                    "the build was stopped for a takeover; building {commit} again after it"
                ));
                held = held.requeue()?;
            }
            Within::TimedOut => unreachable!("build-app sets no time limit"),
        }
    };
    let Some(mac) = config.mac.as_ref().filter(|_| platform == Platform::Mac) else {
        held.release();
        return Ok(());
    };
    let app = copy_path(places, repo, mac)?;
    copy_out(&built_app(&mut site, mac)?, &app)?;
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
