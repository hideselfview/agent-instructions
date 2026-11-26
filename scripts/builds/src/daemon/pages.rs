//! What the page's routes read their answers from, so that one set of routes
//! (http.rs) serves the daemon's live data and, for a shot, a fixture:
//!
//! - `Model` answers from the checkouts and the stores as they are now.
//! - `Fixture` answers from a file that holds the state, the lists, the hooks
//!   and the Designs data of one shot. `builds dashboard stage` serves it
//!   (stage.rs).
//!
//! A fixture reads each answer into the type the daemon sends, so a fixture
//! that no longer fits its type fails when it loads, not as a page the daemon
//! would never send.

use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

use super::designs::{DesignsData, fill_page};
use super::http::DesignsRoute;
use super::lists::Served;
use super::model::{Model, Page};
use crate::fail::{Context, Result};
use crate::hook_stats::Hooks;

/// The data behind the routes `/`, `/api/state`, `/api/lists`, `/api/hooks`,
/// `/events` and `/designs/...`.
pub trait Pages: Send + Sync {
    /// The state the page shows, with the version that `wait_change` waits
    /// past.
    fn state(&self) -> Result<Page>;
    /// The lists JSON. The same `Arc` comes back until the lists change.
    fn lists(&self) -> Result<Arc<String>>;
    /// The hook runs' JSON. The same `Arc` comes back until the runs change.
    fn hooks(&self) -> Arc<String>;
    /// The page or frame that a `/designs/...` route names, drawn now, for an
    /// asker that is <here> on this Mac or not; None when it names nothing.
    fn designs(&self, route: &DesignsRoute, here: bool) -> Option<Result<String>>;
    /// Blocks until the state moves past <seen>, and returns its new version.
    fn wait_change(&self, seen: u64) -> u64;
    /// Tells the data that a page watches (<watching>) or stopped watching,
    /// so a live daemon samples faster while a page is open.
    fn watch(&self, watching: bool);
}

impl Pages for Model {
    fn state(&self) -> Result<Page> {
        self.page()
    }

    fn lists(&self) -> Result<Arc<String>> {
        self.lists_json()
    }

    fn hooks(&self) -> Arc<String> {
        self.hooks_json()
    }

    fn designs(&self, route: &DesignsRoute, here: bool) -> Option<Result<String>> {
        match *route {
            DesignsRoute::Page(project) => self.designs_page(project, here),
            DesignsRoute::Board(project, commit, name) => self.designs_board(project, commit, name),
            DesignsRoute::Component(project, commit, name) => {
                self.designs_preview(project, commit, name)
            }
            DesignsRoute::Colors(project, commit, n) => self.designs_colors(project, commit, n),
            DesignsRoute::ColorsComparison(project, commit) => {
                self.designs_colors_comparison(project, commit)
            }
            DesignsRoute::ComponentShots(project, name) => {
                self.component_shots_page(project, name, here)
            }
            DesignsRoute::Picture(..) => unreachable!("the live routes answer pictures first"),
        }
    }

    fn wait_change(&self, seen: u64) -> u64 {
        Model::wait_change(self, seen)
    }

    fn watch(&self, watching: bool) {
        Model::watch(self, watching)
    }
}

/// A fixture file `<shot>.json`. Its `capture` part is the shots program's
/// (when the page is ready and how to reach the shot's view), and the stage
/// reads only its `stage` part, the answers.
#[derive(Deserialize)]
struct FixtureFile {
    stage: Answers,
}

/// The answers of one staged page, each in the type the daemon sends.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answers {
    state: Page,
    lists: Served,
    hooks: Hooks,
    /// The Designs page's data; a fixture of a page with no Designs page
    /// gives none.
    designs: Option<DesignsData>,
}

/// One shot's answers. The page's state, lists and hooks are the typed
/// answers, held as the text the page gets, because the routes send them
/// again on every request.
pub struct Fixture {
    state: String,
    lists: Arc<String>,
    hooks: Arc<String>,
    designs: Option<DesignsData>,
}

/// The page that a frame of the Designs page loads: nothing, so that what a
/// board or a component draws never differs between a shot and its board.
const EMPTY_FRAME: &str =
    "<!doctype html><html><head><meta charset=\"utf-8\"></head><body></body></html>\n";

impl Fixture {
    /// Reads fixture <name> of <folder> (`<folder>/<name>.json`). Fails when
    /// the file can't be read or an answer does not fit its type.
    pub fn load(folder: &Path, name: &str) -> Result<Fixture> {
        let path = folder.join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path)
            .context(|| format!("reading the fixture {}", path.display()))?;
        let file: FixtureFile = serde_json::from_str(&text).context(|| {
            format!(
                "reading the fixture {} as the daemon's answers",
                path.display()
            )
        })?;
        let Answers {
            state,
            lists,
            hooks,
            designs,
        } = file.stage;
        Ok(Fixture {
            state: serde_json::to_string(&state).expect("the state serializes"),
            lists: Arc::new(serde_json::to_string(&lists).expect("the lists serialize")),
            hooks: Arc::new(serde_json::to_string(&hooks).expect("the hooks serialize")),
            designs,
        })
    }
}

impl Pages for Fixture {
    fn state(&self) -> Result<Page> {
        Ok(serde_json::from_str(&self.state).expect("the state read back as it was written"))
    }

    fn lists(&self) -> Result<Arc<String>> {
        Ok(self.lists.clone())
    }

    fn hooks(&self) -> Arc<String> {
        self.hooks.clone()
    }

    fn designs(&self, route: &DesignsRoute, _here: bool) -> Option<Result<String>> {
        let designs = self.designs.as_ref()?;
        match *route {
            DesignsRoute::Page(project) if project == designs.project => {
                Some(Ok(fill_page(designs)))
            }
            DesignsRoute::Board(project, ..)
            | DesignsRoute::Component(project, ..)
            | DesignsRoute::Colors(project, ..)
            | DesignsRoute::ColorsComparison(project, ..)
                if project == designs.project =>
            {
                Some(Ok(EMPTY_FRAME.to_string()))
            }
            _ => None,
        }
    }

    /// The fixture never changes, so nothing moves past any version.
    fn wait_change(&self, _seen: u64) -> u64 {
        loop {
            std::thread::park();
        }
    }

    fn watch(&self, _watching: bool) {}
}
