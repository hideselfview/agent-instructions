//! Taking the shots of a web app: the app's `shots-stage <app> <command>…`
//! line names the command that serves the page of one shot, and the checker
//! opens that page in the pinned Chrome.
//!
//! For each selected shot the checker:
//!
//! 1. starts the stage command from the repo's top, with `{shot}` in its
//!    arguments replaced by the shot's name. The command prints the page's URL
//!    as the first line of its output, serves the page for that shot on
//!    127.0.0.1, and exits once its input closes;
//! 2. loads the URL in a page of the shot's frame from `canvas.json`
//!    (render/live.rs), which waits for the `ready` selector of the shot's
//!    fixture and runs its `steps`;
//! 3. writes `<name>@web.png` and `<name>@web.json`;
//! 4. closes the stage's input and waits for it to exit.
//!
//! A fixture is the file `<name>.json` in the app's `shots-declared` folder.
//! The checker reads its `capture` part (`live::Capture`) and leaves the rest
//! to the stage:
//!
//! ```json
//! {
//!   "capture": { "ready": "#findings .row", "steps": [{"click": "#show"}] },
//!   "stage": { "…": "what the app's stage reads" }
//! }
//! ```
//!
//! A shot fails, with why, when its stage prints no URL or exits with an
//! error, its ready selector matches nothing within the deadline, or a step's
//! selector matches nothing.

use super::{CaptureError, Failed, Taken, scene_problems, undeclared_problems};
use crate::declared::Declared;
use crate::design;
use crate::names::{App, Apps, Name, Platform, Prefix};
use crate::render::live::{self, Capture};
use crate::render::pages::SCALE;
use crate::render::{self, board, chrome};
use crate::settings::Settings;
use crate::shots::ElementList;
use serde::Deserialize;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Real time a stage gets to exit once its input is closed.
const STAGE_EXIT_DEADLINE: Duration = Duration::from_secs(30);

/// What a capture of a web app's shots needs.
pub(crate) struct Job<'a> {
    pub(crate) root: &'a Path,
    /// The app's `shots-stage` command.
    pub(crate) command: &'a [String],
    pub(crate) declared: &'a [Declared],
    pub(crate) app: &'a App,
    pub(crate) apps: &'a Apps,
    pub(crate) prefixes: &'a [Prefix],
    pub(crate) base: bool,
    /// The absolute folder the shots are written to.
    pub(crate) out: &'a Path,
    pub(crate) settings: &'a Settings,
    pub(crate) folders: &'a boards::Folders,
    /// Real time a stage gets to print its URL, and a page gets to be drawn
    /// and read.
    pub(crate) deadline: Duration,
}

/// Takes the shots of the job's app that its prefixes select (all when none),
/// into `out`. A shot that fails does not fail the capture; it is in
/// `failed`.
pub(crate) fn capture(job: &Job) -> Result<Taken, CaptureError> {
    let taken = super::taken(job.declared, job.app, job.prefixes, false, job.base)?;
    super::clear(job.out, job.app, job.apps)?;
    if taken.names.is_empty() {
        return Ok(taken);
    }
    let command_line = job.command.join(" ");
    if job.command.is_empty() {
        return Err(CaptureError::NoCommand(job.app.name.clone()));
    }
    crate::log::note(&command_line);
    let results = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CaptureError::Chrome(e.to_string()))?
        .block_on(chrome::with_chrome(job.folders, SCALE, async |chrome| {
            let mut results = Vec::new();
            for name in &taken.names {
                results.push(shot(chrome, job, name).await);
            }
            results
        }))
        .map_err(|e| CaptureError::Chrome(e.to_string()))?;
    let declared_here: Vec<Name> = job
        .declared
        .iter()
        .map(|d| d.name.clone())
        .filter(|n| n.app() == job.app)
        .collect();
    let stray = undeclared_problems(job.out, job.app, job.apps, &declared_here)?;
    if !stray.is_empty() {
        return Err(CaptureError::Incomplete(stray));
    }
    let mut names = Vec::new();
    let mut failed = Vec::new();
    for (name, result) in taken.names.into_iter().zip(results) {
        let why = match result {
            Ok(()) => scene_problems(job.out, &name).join("; "),
            Err(why) => why,
        };
        if why.is_empty() {
            names.push(name);
        } else {
            failed.push(Failed { name, why });
        }
    }
    Ok(Taken {
        names,
        not_taken: taken.not_taken,
        failed,
    })
}

/// The part of a fixture the checker reads.
#[derive(Deserialize)]
struct Fixture {
    capture: Capture,
}

/// Takes the shot <name>: stages its page, draws it and writes its files.
async fn shot(chrome: &chrome::Chrome, job: &Job<'_>, name: &Name) -> Result<(), String> {
    let declared = job
        .declared
        .iter()
        .find(|d| &d.name == name)
        .ok_or_else(|| format!("{name} is declared by no fixture"))?;
    let text = std::fs::read_to_string(&declared.file)
        .map_err(|e| format!("cannot read {}: {e}", declared.file.display()))?;
    let fixture: Fixture =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", declared.file.display()))?;
    let frame = design::frame(job.root, &name.to_string()).map_err(|e| e.to_string())?;
    let stage = Stage::start(job.root, job.command, name, job.deadline).await?;
    let taken = chrome
        .with_page_within(&format!("shot {name}"), job.deadline, async |page| {
            live::load(
                page,
                &stage.url,
                frame,
                &fixture.capture,
                job.folders.fonts.as_deref(),
            )
            .await?;
            let png = board::screenshot(page).await.map_err(|e| e.to_string())?;
            let bounds = board::frame_bounds(frame);
            let read = render::read_page(page, job.settings, bounds, Platform::Web).await?;
            Ok((png, bounds, read))
        })
        .await;
    // The stage ends whether or not its page was drawn.
    let ended = stage.end().await;
    let (png, bounds, read) = taken?;
    ended?;
    let list = ElementList {
        board: name.to_string(),
        platform: Platform::Web,
        size: crate::shots::Size {
            width: bounds.width,
            height: bounds.height,
        },
        scale: chrome.scale(),
        elements: read.elements,
        unshot: read.unshot,
        slots: read.slots,
        window: None,
    };
    write(job.out, &name.file_name("png"), &png)?;
    write(
        job.out,
        &name.file_name("json"),
        &serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?,
    )
}

fn write(out: &Path, file: &str, bytes: &[u8]) -> Result<(), String> {
    let path = out.join(file);
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// A running stage command and the URL it printed.
struct Stage {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    output: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    url: String,
}

impl Stage {
    /// Starts <command> from <root> for the shot <name> and reads the URL on
    /// the first line of its output, which it has <deadline> to print.
    async fn start(
        root: &Path,
        command: &[String],
        name: &Name,
        deadline: Duration,
    ) -> Result<Self, String> {
        let shot = name.to_string();
        let words: Vec<String> = command
            .iter()
            .map(|word| word.replace("{shot}", &shot))
            .collect();
        let (program, args) = words.split_first().expect("the command has a program");
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot start the stage {}: {e}", words.join(" ")))?;
        let stdin = child.stdin.take().expect("the stage's input is piped");
        let stdout = child.stdout.take().expect("the stage's output is piped");
        let mut output = BufReader::new(stdout).lines();
        let line = tokio::time::timeout(deadline, output.next_line())
            .await
            .map_err(|_| {
                format!(
                    "the stage printed no URL within {} seconds",
                    deadline.as_secs()
                )
            })?
            .map_err(|e| format!("cannot read the stage's output: {e}"))?;
        let Some(url) = line else {
            let status = child
                .wait()
                .await
                .map_err(|e| format!("cannot wait for the stage: {e}"))?;
            return Err(format!("the stage printed no URL and exited: {status}"));
        };
        if !url.starts_with("http://127.0.0.1:") {
            return Err(format!(
                "the stage's first line is {url:?}, not a URL on 127.0.0.1"
            ));
        }
        Ok(Self {
            child,
            stdin,
            output,
            url,
        })
    }

    /// Closes the stage's input and waits for it to exit.
    async fn end(self) -> Result<(), String> {
        let Self {
            mut child,
            stdin,
            mut output,
            ..
        } = self;
        drop(stdin);
        // The stage may print more, and waits on a reader that does not read.
        let drained = async { while let Ok(Some(_)) = output.next_line().await {} };
        let waited = tokio::time::timeout(STAGE_EXIT_DEADLINE, async {
            tokio::join!(child.wait(), drained).0
        })
        .await
        .map_err(|_| {
            format!(
                "the stage did not exit within {} seconds of its input closing",
                STAGE_EXIT_DEADLINE.as_secs()
            )
        })?
        .map_err(|e| format!("cannot wait for the stage: {e}"))?;
        if waited.success() {
            Ok(())
        } else {
            Err(format!("the stage exited with an error: {waited}"))
        }
    }
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
