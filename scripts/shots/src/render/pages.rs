//! The pages of a design that no shot of a run draws: a board with no shot
//! name (a shared board) and a component's whole preview page. `draws` and
//! `states` draw them through the picture folder, which keeps what each page
//! drew by the hash of everything it depends on (design.rs): its picture and
//! element list (pictures.rs), and the components it draws.

use crate::design::{self, Common, DesignSystem, Frame};
use crate::names::{Platform, Previews};
use crate::pictures::Pictures;
use crate::render::instances::{self, Instance};
use crate::render::runtime;
use crate::render::serve::Tree;
use crate::render::{board, chrome, preview, read, serve};
use crate::settings::Settings;
use crate::shots::{Element, ElementList, Size};
use boards::runtime::RuntimeCopy;
use std::collections::BTreeMap;
use std::path::Path;

/// The pixels per point that pages draw at and that their hashes name, the
/// device scale of a Mac shot: Chrome rounds boxes and lines to the device's
/// pixels, so a page that draws alike at this scale draws alike in a shots
/// run.
pub(crate) const SCALE: f64 = 2.0;

/// The picture folder a comparison takes pages from and keeps the ones it
/// draws in, with the capture version of the checker that keys them.
pub(crate) struct Store<'a> {
    pub(crate) capture_version: &'a str,
    pub(crate) pictures: Option<&'a Pictures>,
}

/// What draws the pages of the design tree at one root: the repo's settings,
/// its previews, the canvas runtime copy and the design system built from it,
/// and every board's frame.
pub(crate) struct Context {
    pub(crate) settings: Settings,
    pub(crate) previews: Previews,
    pub(crate) runtime: &'static RuntimeCopy,
    pub(crate) design: DesignSystem,
    pub(crate) frames: BTreeMap<String, Frame>,
}

impl Context {
    pub(crate) fn of(root: &Path, folders: &boards::Folders) -> Result<Self, String> {
        let settings = Settings::load(root).map_err(|e| e.to_string())?;
        let previews = Previews::of(root)?;
        let runtime = runtime::check(root).map_err(|e| e.to_string())?.copy;
        let design = DesignSystem::of(root, folders).map_err(|e| e.to_string())?;
        let frames = design::frames(root).map_err(|e| e.to_string())?;
        Ok(Self {
            settings,
            previews,
            runtime,
            design,
            frames,
        })
    }

    pub(crate) fn tree<'a>(&'a self, root: &'a Path) -> Tree<'a> {
        Tree {
            root,
            design: &self.design,
            runtime: self.runtime.text,
        }
    }

    fn common<'a>(&'a self, version: &'a str) -> Common<'a> {
        Common {
            design: &self.design,
            browser_version: chrome::PINNED,
            program_version: version,
            settings: &self.settings,
            scale: SCALE,
        }
    }
}

/// A page: a board, by name, in its frame, or a component's preview page.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Page<'a> {
    Board(&'a str, Frame),
    Preview(&'a str),
}

impl Page<'_> {
    /// The hash of everything the page's picture and instances depend on.
    pub(crate) fn hash(
        &self,
        root: &Path,
        context: &Context,
        capture_version: &str,
    ) -> Result<String, String> {
        let common = context.common(capture_version);
        match self {
            Page::Board(name, _) => design::board_hash(root, name, context.runtime.sha256, &common),
            Page::Preview(component) => design::preview_page_hash(root, component, &common),
        }
        .map_err(|e| e.to_string())
    }
}

impl Page<'_> {
    /// What an error names the page as.
    fn what(&self) -> String {
        match self {
            Page::Board(name, _) => format!("board {name}"),
            Page::Preview(component) => format!("the preview of {component}"),
        }
    }

    /// Loads the page in <tab>, drawn when this returns.
    async fn load(&self, tab: &chrome::DrawPage, server: &serve::Server) -> Result<(), String> {
        match self {
            Page::Board(name, frame) => {
                board::load(
                    tab,
                    name,
                    &server.board_url(name),
                    *frame,
                    board::BOARD_MOUNTED,
                    None,
                )
                .await
            }
            Page::Preview(component) => {
                board::load(
                    tab,
                    component,
                    &server.preview_url(component),
                    preview::PREVIEW_FRAME,
                    preview::PREVIEW_LOADED,
                    None,
                )
                .await
            }
        }
        .map_err(|e| e.to_string())
    }

    /// The components the loaded page draws in <tab>; a preview's rows are
    /// the ones the settings' marks select.
    async fn instances(
        &self,
        tab: &chrome::DrawPage,
        context: &Context,
    ) -> Result<Vec<Instance>, String> {
        let rows = match self {
            Page::Board(..) => None,
            Page::Preview(_) => context
                .settings
                .preview
                .as_ref()
                .map(|marks| marks.row.as_str()),
        };
        instances::instances(tab, rows)
            .await
            .map_err(|e| e.to_string())
    }
}

/// What a page drew: its picture, its elements (read as the Mac's), its size
/// in points and the components it draws.
pub(crate) struct Drawn {
    pub(crate) png: Vec<u8>,
    pub(crate) size: Size,
    pub(crate) elements: Vec<Element>,
    pub(crate) instances: Vec<Instance>,
}

impl Drawn {
    /// The element list the picture folder keeps beside the picture, for the
    /// page named <label>.
    pub(crate) fn list(&self, label: &str) -> ElementList {
        ElementList {
            board: label.to_string(),
            platform: Platform::Mac,
            size: self.size,
            scale: SCALE,
            elements: self.elements.clone(),
            unshot: Vec::new(),
            slots: Vec::new(),
            window: None,
        }
    }
}

/// Draws <page> in <chrome>, which has the device scale `SCALE`.
pub(crate) async fn draw(
    page: Page<'_>,
    context: &Context,
    server: &serve::Server,
    chrome: &chrome::Chrome,
) -> Result<Drawn, String> {
    chrome
        .with_page(&page.what(), async |tab| {
            page.load(tab, server).await?;
            let (png, frame) = match page {
                Page::Board(_, frame) => (
                    board::screenshot(tab).await.map_err(|e| e.to_string())?,
                    board::frame_bounds(frame),
                ),
                Page::Preview(_) => {
                    let rendered = preview::capture(tab, preview::CaptureArea::Page)
                        .await
                        .map_err(|e| e.to_string())?;
                    (rendered.png, rendered.frame)
                }
            };
            let elements = read::elements(tab, frame, Platform::Mac)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Drawn {
                png,
                size: Size {
                    width: frame.width,
                    height: frame.height,
                },
                elements,
                instances: page.instances(tab, context).await?,
            })
        })
        .await
}

/// The components <page> draws, read from a drawing of it in <chrome>.
pub(crate) async fn read_instances(
    page: Page<'_>,
    context: &Context,
    server: &serve::Server,
    chrome: &chrome::Chrome,
) -> Result<Vec<Instance>, String> {
    chrome
        .with_page(&page.what(), async |tab| {
            page.load(tab, server).await?;
            page.instances(tab, context).await
        })
        .await
}
