//! `builds build-app <repo> <app> [<ref>]`: builds the person's test app
//! in an app checkout of the platform's lane (lane.rs), whose build stays
//! warm, taking turns on that lane's app lock. Nothing is opened. The Mac
//! build takes a checkout on this Mac or on a host, as `builds app-build`
//! does, except when its command is a step that a `here` line names, which
//! takes only a checkout on this Mac, and a build on a host copies the built app back here. The Mac app is
//! copied out to `~/Applications/<repo>-test`, since the next build there
//! replaces it, in place of the copy there, the running one quit first. The
//! Android build is the repo's own script, which installs the app where it
//! should go, so it takes only a checkout on this Mac.
//!
//! A stop signal (SIGTERM, SIGINT or SIGHUP) ends the command from the moment
//! it starts (lock.rs `end_on_stop_signals`): it ends the wait for the lane's
//! lock, the build that runs (on the host too) and the wait for the running
//! app to quit, starts nothing after it, and fails, naming the signal. The
//! lane's lock goes as the command ends. It then exits with the status a
//! shell gives a program that the signal ended (128 plus the signal), which
//! the dashboard reads as stopped (daemon/runs.rs).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use crate::app_build::{config_at, place_of};
use crate::app_config::{CONFIG, Config, CopiedApp, Step, run_command};
use crate::app_lock_guard::CheckoutLock;
use crate::app_platform::{self, Install};
use crate::app_site::Site;
use crate::fail;
use crate::fail::{Context, Result};
use crate::host::Ssh;
use crate::lane::Lane;
use crate::lock::{self, Where, Within};
use crate::places::Places;
use crate::text::note;
use crate::tool::Tool;

const USAGE: &str = "usage: builds build-app <repo> <app> [<ref>]";
const QUIT_CHECKS: u32 = 60;
const QUIT_CHECK_EVERY: Duration = Duration::from_millis(500);

/// The lane `build-app` builds and what it does with the built app, which
/// the lane's platform names (app_platform.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    lane: Lane,
    install: Install,
}

impl Target {
    /// The target of the app <word> names among the apps of <config>.
    fn parse(config: &Config, word: &str) -> Result<Target> {
        Ok(Target::of(Lane::parse_in_or_fail(&config.lanes, word)?))
    }

    fn of(lane: Lane) -> Target {
        Target {
            lane,
            install: app_platform::of(lane).install(),
        }
    }

    fn lane(self) -> Lane {
        self.lane
    }

    /// Which of the lane's checkouts the build may take: the Mac build, on
    /// any host's too, since the build copies its app back here, unless its
    /// command is one a `here` line names (a step only this Mac can run, such
    /// as the signed app); the Android build installs the app on the device
    /// of the Mac it runs on.
    fn allowed(self, config: &Config) -> Result<Where> {
        match self.install {
            Install::CopiedApp => {
                let build = self.build(config)?;
                let named: Vec<Step> = config
                    .steps
                    .iter()
                    .filter(|step| step.command == build)
                    .cloned()
                    .collect();
                Ok(place_of(&named))
            }
            Install::OnDevice => Ok(Where::Here),
            Install::None => fail!(
                "{} is a web app, which has no build-app: its shots stage their pages",
                self.word()
            ),
        }
    }

    fn word(&self) -> &str {
        self.lane.word()
    }

    fn build(self, config: &Config) -> Result<&[String]> {
        match config.test_app_of(self.lane) {
            Some(install) => Ok(&install.build),
            None => fail!("{CONFIG} has no {} lines", self.word()),
        }
    }
}

/// The `app` and `bundle` lines of <lane>, which a build that copies its app
/// out has.
fn copied_app(config: &Config, lane: Lane) -> Result<&CopiedApp> {
    match config
        .test_app_of(lane)
        .and_then(|test| test.copied.as_ref())
    {
        Some(copied) => Ok(copied),
        None => fail!(
            "{CONFIG} has no build, app and bundle lines for {}",
            lane.word()
        ),
    }
}

/// Where the test app lives: outside the checkout, under the home folder.
fn copy_path(places: &Places, repo: &str, lane: Lane, built: &CopiedApp) -> Result<PathBuf> {
    let Some(name) = Path::new(&built.app).file_name() else {
        fail!("{CONFIG}: {} app names no file", lane.word());
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
fn copy_out(built: &Path, app: &Path, checkout: CheckoutLock<'_>) -> Result<()> {
    let folder = app.parent().expect("the test app has a folder");
    std::fs::create_dir_all(folder).context(|| format!("creating {}", folder.display()))?;
    let copy = with_new(app);
    if copy.exists() {
        std::fs::remove_dir_all(&copy).context(|| format!("removing {}", copy.display()))?;
    }
    let mut command = Command::new("ditto");
    command.arg(built).arg(&copy);
    let status = checkout.run(command, |mut command| {
        command.status().context(|| "running ditto".to_string())
    })?;
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
        lock::refuse_when_stopped(&format!(
            "waited for {bundle} to quit; the new app is at {}",
            with_new(app).display()
        ))?;
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

fn parse_args(args: &[String]) -> Result<(&String, &str, &str)> {
    match args {
        [repo, app] => Ok((repo, app, DEFAULT_REF)),
        [repo, app, commit] if !commit.starts_with('-') => Ok((repo, app, commit.as_str())),
        _ => fail!("{USAGE}"),
    }
}

/// Runs the build <command> in <site>, with no time limit: here, or on the
/// host through `builds remote step`.
fn run_build(site: &mut Site, command: &[String]) -> Result<Within> {
    match site {
        Site::Local(top, _, checkout) => run_command(top, command, *checkout),
        Site::Remote(remote) => remote.run_step(command, &command[0], None, lock::Keep::default()),
    }
}

/// The folder on this Mac that holds the built app of <mac>: the checkout
/// itself, or for a checkout on a host the folder the build's output was
/// copied to.
fn built_app(site: &mut Site, built: &CopiedApp) -> Result<PathBuf> {
    match site {
        Site::Local(top, ..) => Ok(top.join(&built.app)),
        Site::Remote(remote) => {
            remote.pull_bundle(&built.app)?;
            Ok(remote.local_folder().join(&built.app))
        }
    }
}

pub fn cmd_build_app(tool: &Tool, args: &[String]) -> Result<()> {
    ended(build_app(tool, args), lock::stop_signal())
}

/// What <result> exits with, given the stop <signal> the command got, if
/// any: 128 plus the signal when it ended the command, and 1 for every other
/// failure, whatever status the build command had. The dashboard reads a
/// status above 128 as stopped (daemon/runs.rs), so a build that crashed
/// must not exit with one.
fn ended(result: Result<()>, signal: Option<libc::c_int>) -> Result<()> {
    match (result, signal) {
        (Err(fail), Some(signal)) => {
            note(fail.text());
            Err(fail::Fail::Exit(128 + signal))
        }
        (Err(fail), None) => {
            note(fail.text());
            Err(fail::Fail::Exit(1))
        }
        (Ok(()), _) => Ok(()),
    }
}

fn build_app(tool: &Tool, args: &[String]) -> Result<()> {
    let (repo, word, commit) = parse_args(args)?;
    lock::end_on_stop_signals();
    let places = &tool.places;
    let ssh = Ssh::from_env();
    let first_config = config_at(places, repo, commit)?;
    let platform = Target::parse(&first_config, word)?;

    note(format!(
        "waiting for {repo}'s {} app lock, then building {commit}'s {} app",
        platform.word(),
        platform.word()
    ));
    let allowed = platform.allowed(&first_config)?;
    if allowed == Where::Here {
        crate::worktree::ensure_local_app_checkout(tool, &ssh, repo, platform.lane())?;
    }
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
    let (bundle, app) = loop {
        let mut site = Site::of(
            places,
            &ssh,
            repo,
            platform.lane(),
            &held.checkout,
            held.checkout_lock(),
            &tool.changed,
        )?;
        note(format!("building in {}", held.checkout));
        let config = site.check_out(places, repo, commit)?.config;
        let ran = run_build(&mut site, platform.build(&config)?);
        lock::refuse_when_stopped(&format!("built {commit}'s {} app", platform.word()))?;
        match ran? {
            Within::Ran(lock::Ran::Finished) => {
                if platform.install != Install::CopiedApp {
                    drop(site);
                    held.release();
                    return Ok(());
                }
                let built = copied_app(&config, platform.lane())?;
                let app = copy_path(places, repo, platform.lane(), built)?;
                copy_out(&built_app(&mut site, built)?, &app, held.checkout_lock())?;
                break (built.bundle.clone(), app);
            }
            // Only a takeover stops it: it holds no use of the stores, so
            // install never stops it.
            Within::Ran(lock::Ran::Stopped) => {
                note(format!(
                    "the build was stopped for a takeover; building {commit} again after it"
                ));
                drop(site);
                held = held.requeue()?;
            }
            Within::TimedOut => unreachable!("build-app sets no time limit"),
        }
    };
    lock::refuse_when_stopped(&format!(
        "copied the built app to {}",
        with_new(&app).display()
    ))?;
    // The build and the copy are done: the next build can start.
    held.release();

    quit(&bundle, &app)?;
    replace(&app)?;
    note(format!("built {}", app.display()));
    Ok(())
}

#[cfg(test)]
#[path = "build_app_tests.rs"]
mod tests;
