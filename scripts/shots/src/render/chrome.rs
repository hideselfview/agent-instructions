//! The pinned Chrome for Testing build the boards render in.

use chromiumoxide::Page;
use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Real time one board or preview may take to draw on its page.
pub(crate) const DRAW_DEADLINE: Duration = Duration::from_secs(120);
/// Real time Chrome gets to close a page, to answer a question about a page
/// whose draw ran out of time, and to close.
const CLOSE_DEADLINE: Duration = Duration::from_secs(10);

/// The Chrome for Testing build every board hash names.
pub(crate) const PINNED: &str = "157.0.8103.78";

/// A platform Chrome for Testing is published for.
#[derive(Debug, PartialEq, Eq)]
struct Platform {
    /// The platform's name in the download URL and the archive.
    word: &'static str,
    /// The browser's executable inside the unpacked archive.
    executable: &'static str,
    /// The sha256 of the pinned version's archive.
    sha256: &'static str,
}

impl Platform {
    fn of(os: &str, arch: &str) -> Option<Self> {
        match (os, arch) {
            ("macos", "aarch64") => Some(Self {
                word: "mac-arm64",
                sha256: "bf3b49d0fe62a62397e98641aa736ef813c46014f03ec13d4913442e6966c7fc",
                executable: "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            }),
            ("macos", "x86_64") => Some(Self {
                word: "mac-x64",
                sha256: "8f9590a4e011ba4ea135f50c711f8bf2c7f32b1fac20de68d1b85f010f095ab3",
                executable: "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
            }),
            ("linux", "x86_64") => Some(Self {
                word: "linux64",
                executable: "chrome-linux64/chrome",
                sha256: "9223832977a13db3646201b1341c1f9cf08afa3ceae92540116d7b0c062364fb",
            }),
            _ => None,
        }
    }

    fn url(&self, version: &str) -> String {
        format!(
            "https://storage.googleapis.com/chrome-for-testing-public/{version}/{0}/chrome-{0}.zip",
            self.word
        )
    }

    /// The folder `version` unpacks into under the cache.
    fn folder(&self, cache: &Path, version: &str) -> PathBuf {
        cache.join(format!("{version}-{}", self.word))
    }
}

/// The pinned browser's executable, downloaded and unpacked into the cache
/// when it is not there. Each run unpacks in a folder of its own and renames
/// it into place, so the folder is whole or absent; of two runs that fetch at
/// once, the one that renames second finds the build there and uses it.
/// The builds are kept in `chrome` in the program's cache folder <cache>.
fn installed(cache: &Path) -> Result<PathBuf, ChromeError> {
    let platform = Platform::of(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        ChromeError::Fetch(format!(
            "no Chrome for Testing for {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let cache = cache.join("chrome");
    let fetch = |e: std::io::Error| ChromeError::Fetch(e.to_string());
    std::fs::create_dir_all(&cache).map_err(fetch)?;
    let folder = platform.folder(&cache, PINNED);
    let executable = folder.join(platform.executable);
    if executable.is_file() {
        return Ok(executable);
    }
    let work = tempfile::tempdir_in(&cache).map_err(fetch)?;
    let archive = work.path().join("chrome.zip");
    let unpacked = work.path().join("unpacked");
    let (archive_arg, unpacked_arg) = (archive.to_string_lossy(), unpacked.to_string_lossy());
    run(
        "download Chrome for Testing",
        &cache,
        "curl",
        &[
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
            &archive_arg,
            &platform.url(PINNED),
        ],
    )?;
    verify(&archive, platform.sha256)?;
    run(
        "unpack Chrome for Testing",
        &cache,
        "unzip",
        &["-q", &archive_arg, "-d", &unpacked_arg],
    )?;
    match std::fs::rename(&unpacked, &folder) {
        Ok(()) => Ok(executable),
        Err(_) if executable.is_file() => Ok(executable),
        Err(e) => Err(fetch(e)),
    }
}

/// Runs <program> with <args> in <cwd>, failing with <step> named.
fn run(step: &str, cwd: &Path, program: &str, args: &[&str]) -> Result<(), ChromeError> {
    crate::log::note(step);
    let status = std::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|e| ChromeError::Fetch(format!("cannot start {step}: {e}")))?;
    if status.success() {
        return Ok(());
    }
    Err(ChromeError::Fetch(format!("{step} failed: {status}")))
}

/// Fails unless the file at `archive` hashes to `expected`.
fn verify(archive: &Path, expected: &str) -> Result<(), ChromeError> {
    let fetch = |e: std::io::Error| ChromeError::Fetch(format!("{}: {e}", archive.display()));
    let mut hasher = Sha256::new();
    std::io::copy(
        &mut std::fs::File::open(archive).map_err(fetch)?,
        &mut hasher,
    )
    .map_err(fetch)?;
    let actual: String = crate::hex::hex(&hasher.finalize());
    if actual == expected {
        return Ok(());
    }
    Err(ChromeError::Checksum {
        expected: expected.to_string(),
        actual,
    })
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ChromeError {
    #[error(
        "the downloaded Chrome for Testing archive hashes to {actual} but the pinned build is \
{expected}"
    )]
    Checksum { expected: String, actual: String },
    #[error("cannot fetch Chrome for Testing: {0}")]
    Fetch(String),
    #[error(
        "Chrome reports {actual}, not the pinned {expected}; board hashes name the pinned build"
    )]
    Version { expected: String, actual: String },
    #[error("cannot start Chrome: {0}")]
    Launch(String),
    /// Chrome ran and printed no debugging address in its deadline.
    #[error("cannot start Chrome: {0}")]
    NoAddress(String),
    #[error("Chrome failed: {0}")]
    Protocol(String),
}

pub(crate) struct Chrome {
    browser: Browser,
    /// What the browser reports as its product, e.g. `Chrome/157.0.8103.78`.
    version: String,
}

impl Chrome {
    pub(crate) async fn new_page(&self) -> Result<Page, ChromeError> {
        self.browser
            .new_page("about:blank")
            .await
            .map_err(|e| ChromeError::Protocol(e.to_string()))
    }

    /// Runs <work> on a new page and closes the page after it, whatever
    /// <work> returns; a page that fails to close is noted. Every error names
    /// <what>, the board or preview being drawn. Chrome gets `DRAW_DEADLINE`
    /// to open the page and finish <work>.
    pub(crate) async fn with_page<T>(
        &self,
        what: &str,
        work: impl AsyncFnOnce(&Page) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_page_within(what, DRAW_DEADLINE, work).await
    }

    pub(crate) async fn with_page_within<T>(
        &self,
        what: &str,
        deadline: Duration,
        work: impl AsyncFnOnce(&Page) -> Result<T, String>,
    ) -> Result<T, String> {
        let end = tokio::time::Instant::now() + deadline;
        let late = |doing: &str, last: String| {
            format!(
                "{what}: Chrome did not finish within {} seconds, while {doing}{last}",
                deadline.as_secs()
            )
        };
        let page = tokio::time::timeout_at(end, self.new_page())
            .await
            .map_err(|_| late("opening its page", String::new()))?
            .map_err(|e| format!("{what}: {e}"))?;
        let out = match tokio::time::timeout_at(end, work(&page)).await {
            Ok(out) => out.map_err(|e| format!("{what}: {e}")),
            Err(_) => Err(late("drawing it", last_said(&page).await)),
        };
        note_unless_done(&format!("closing the page of {what}"), page.close()).await;
        out
    }
}

/// Runs <step> for at most `CLOSE_DEADLINE`, and notes that <doing> failed,
/// with the error, or took too long.
async fn note_unless_done<T, E: std::fmt::Display>(
    doing: &str,
    step: impl Future<Output = Result<T, E>>,
) {
    match tokio::time::timeout(CLOSE_DEADLINE, step).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => crate::log::note(format_args!("{doing} failed: {error}")),
        Err(_) => crate::log::note(format_args!(
            "{doing} took over {} seconds",
            CLOSE_DEADLINE.as_secs()
        )),
    }
}

/// What <page> still answers after its draw ran out of time: its address and
/// how far the board's watch script got, or the error of each question that
/// failed, or that Chrome answered none within `CLOSE_DEADLINE`.
async fn last_said(page: &Page) -> String {
    let ask = async {
        let url = match page.url().await {
            Ok(Some(url)) => url,
            Ok(None) => "no address".to_string(),
            Err(error) => format!("an address that failed to read ({error})"),
        };
        let state = match page.evaluate("window.__buildsShots.state").await {
            Ok(result) => match result.into_value::<String>() {
                Ok(state) => format!("reached '{state}'"),
                Err(error) => format!("a state that failed to read ({error})"),
            },
            Err(error) => format!("a state that failed to read ({error})"),
        };
        format!("; the page at {url} had {state}")
    };
    match tokio::time::timeout(CLOSE_DEADLINE, ask).await {
        Ok(said) => said,
        Err(_) => format!(
            "; Chrome answered no question about the page within {} seconds",
            CLOSE_DEADLINE.as_secs()
        ),
    }
}

/// How long Chrome has to print the address of its debugging socket. On macOS
/// its main thread waits in its registration with the window server, which
/// took 17 to 43 seconds on a busy build host with or without `ARGS`;
/// chromiumoxide gives up after 20.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(120);

/// How Chrome starts: sRGB colour, no scrollbars, and software rasterization on
/// one thread with no partial raster, checker imaging or LCD text. GPU and
/// multi-threaded rasterization put single pixels of a gradient or an edge a
/// level or more off from one run to the next while the machine is busy.
/// Starting the compositor on one thread or running all its stages before each
/// draw makes screenshots time out. chromiumoxide puts the `--` in front of each argument itself.
const ARGS: [&str; 7] = [
    "force-color-profile=srgb",
    "hide-scrollbars",
    "disable-gpu",
    "num-raster-threads=1",
    "disable-partial-raster",
    "disable-checker-imaging",
    "disable-lcd-text",
];

/// What starting the pinned Chrome came to (`start`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Start {
    /// Chrome started; its version.
    Started(String),
    /// Chrome ran and printed no debugging address in its deadline: on macOS,
    /// a window server that does not answer.
    NoAddress(String),
    /// Chrome did not start for another reason.
    Failed(String),
}

impl Start {
    fn of(started: Result<String, ChromeError>) -> Start {
        match started {
            Ok(version) => Start::Started(version),
            Err(error @ ChromeError::NoAddress(_)) => Start::NoAddress(error.to_string()),
            Err(error) => Start::Failed(error.to_string()),
        }
    }

    /// The line `builds shots chrome` prints, which builds reads
    /// (`host_chrome::parse`): `chrome=started <version>`,
    /// `chrome=no-address <why>` or `chrome=failed <why>`.
    pub(crate) fn line(&self) -> String {
        match self {
            Start::Started(version) => format!("chrome=started {version}"),
            Start::NoAddress(why) => format!("chrome=no-address {}", one_line(why)),
            Start::Failed(why) => format!("chrome=failed {}", one_line(why)),
        }
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Starts the pinned Chrome from the cache folder <cache>, as a run does, and
/// closes it.
pub(crate) async fn start(cache: &Path) -> Start {
    Start::of(with_chrome(cache, async |chrome| chrome.version.clone()).await)
}

/// Fetches the pinned build into the program's cache folder <cache> when it
/// does not hold it, starts it
/// headless with `ARGS`, runs `work` with it and closes
/// it. The browser's events are driven alongside `work` and end with it, so
/// no task outlives the call.
pub(crate) async fn with_chrome<T>(
    cache: &Path,
    work: impl AsyncFnOnce(&Chrome) -> T,
) -> Result<T, ChromeError> {
    with_executable(installed(cache)?, LAUNCH_DEADLINE, work).await
}

/// `with_chrome` for the browser at <executable>, which has <launch_deadline>
/// to print its debugging address.
async fn with_executable<T>(
    executable: PathBuf,
    launch_deadline: Duration,
    work: impl AsyncFnOnce(&Chrome) -> T,
) -> Result<T, ChromeError> {
    let profile = tempfile::tempdir().map_err(|e| ChromeError::Launch(e.to_string()))?;
    let config = BrowserConfig::builder()
        .user_data_dir(profile.path())
        .chrome_executable(executable)
        .viewport(None)
        .args(ARGS)
        .launch_timeout(launch_deadline)
        .build()
        .map_err(ChromeError::Launch)?;
    let (browser, mut events) = Browser::launch(config).await.map_err(|e| {
        let text = e.to_string();
        if matches!(e, chromiumoxide::error::CdpError::LaunchTimeout(_)) {
            ChromeError::NoAddress(format!(
                "Chrome printed no debugging address in {} seconds. On macOS its main thread waits for the window server to answer; when the window server does not, every AppKit program on the host hangs the same way, Chrome and chrome-headless-shell alike ({text})",
                launch_deadline.as_secs()
            ))
        } else {
            ChromeError::Launch(text)
        }
    })?;
    let running = async move {
        let version = browser
            .version()
            .await
            .map_err(|e| ChromeError::Protocol(e.to_string()))?
            .product;
        let mut chrome = Chrome { browser, version };
        let out = if chrome.version.contains(PINNED) {
            Ok(work(&chrome).await)
        } else {
            Err(ChromeError::Version {
                expected: PINNED.to_string(),
                actual: chrome.version.clone(),
            })
        };
        note_unless_done("closing Chrome", chrome.browser.close()).await;
        note_unless_done("waiting for Chrome", chrome.browser.wait()).await;
        out
    };
    // The events end when the browser closes, which `running` does last; a
    // browser that dies before that fails the work's requests.
    let driven = async {
        while events.next().await.is_some() {}
        std::future::pending::<std::convert::Infallible>().await
    };
    tokio::select! {
        out = running => out,
        never = driven => match never {},
    }
}

#[cfg(test)]
#[path = "chrome_tests.rs"]
mod tests;
