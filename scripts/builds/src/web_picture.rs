//! `builds web picture <url>`: the picture of a web page, which is how an
//! agent sees a web page it changes (`builds help`, WEB PICTURES). The shots
//! checker's pinned Chrome draws the page on this Mac (scripts/shots, the
//! library's `web_picture`), from the cache folder a shots run keeps it in,
//! with a profile folder in the run's temporary folder (run_tmp.rs), on a
//! page that takes one of the host's Chrome slots. The command takes no app
//! checkout, no place in a lane's queue and no window lock. It waits as a
//! shots step on this Mac waits: while the display gives a reason
//! (host_display.rs `hold`), and while Chrome prints no address
//! (host_chrome.rs `wait_until_starts`).
//!
//! The pictures are in the capped store `web-pictures` on the build volume
//! (capped_store.rs), one folder for each picture.

use std::path::{Path, PathBuf};

use shots::{WEB_SETTLE_LIMIT, WebPage, WebPicture, WebPictureError};

use crate::args::Args;
use crate::capped_store::{self, Group, Kept, Use};
use crate::fail;
use crate::fail::{Context, Result};
use crate::host::Host;
use crate::host_chrome::Start;
use crate::output::note;
use crate::places::Places;
use crate::prune::Run;
use crate::system::System;
use crate::tool::Tool;

/// The width of the window a page loads in without `--width`, in CSS pixels.
const WIDTH: u32 = 1440;

/// The folder on the build volume that holds the pictures.
pub const FOLDER: &str = "web-pictures";

/// How much the folder of the pictures may hold. A picture takes from tens of
/// KiB to a few MiB, so the cap holds the pictures of days of work.
pub const CAP_KIB: u64 = 1024 * 1024;

/// The picture in its folder.
const PICTURE: &str = "page.png";

/// The page that the words after `builds web picture` ask for.
pub fn parse(args: &Args) -> Result<WebPage<'_>> {
    let [url] = args.exactly(1)? else {
        unreachable!("`exactly` gives the one word");
    };
    let width = match args.value("width")? {
        None => WIDTH,
        Some(value) => match value.parse() {
            Ok(width) if width > 0 => width,
            _ => fail!(
                "--width takes a whole number of CSS pixels above 0, not {value}\n{}",
                args.command().usage_text()
            ),
        },
    };
    if !is_web(url) {
        fail!("builds web picture takes an http or https URL, and {url} is neither");
    }
    Ok(WebPage {
        url,
        width,
        full_page: args.switch("full-page"),
    })
}

/// Whether <url> is an http or https URL with something after its scheme.
fn is_web(url: &str) -> bool {
    url.split_once("://").is_some_and(|(scheme, rest)| {
        ["http", "https"]
            .iter()
            .any(|web| scheme.eq_ignore_ascii_case(web))
            && !rest.is_empty()
    })
}

/// The picture of <page>, drawn by the pinned Chrome kept in <folders>. Holds
/// the drawing while Chrome prints no address on this Mac.
pub fn draw(folders: &boards::Folders, page: &WebPage) -> Result<WebPicture> {
    // Each try starts Chrome and, once it starts, draws the page.
    let drawn = crate::host_chrome::wait_until_starts(
        &Host::local(),
        &|| {
            Ok(match shots::web_picture(folders, page) {
                Ok(picture) => Start::Started(Ok(picture)),
                Err(WebPictureError::Page(why)) => Start::Started(Err(why)),
                Err(WebPictureError::NoAddress(why)) => Start::NoAddress(why),
                Err(WebPictureError::Chrome(why)) => Start::Failed(why),
            })
        },
        &|words| note(words),
        // No lane's lock records the wait.
        &|_| Ok(()),
        &crate::lock::pause_before_trying_again,
    )?;
    match drawn {
        Some(Ok(picture)) => Ok(picture),
        Some(Err(why)) => fail!("cannot picture {why}"),
        None => fail!("a signal stopped the wait for Chrome to start"),
    }
}

/// The folder of the pictures on the volume of <places>.
pub fn root(places: &Places) -> PathBuf {
    places.root.join(FOLDER)
}

/// Keeps <png> in a folder of its own and returns its path.
pub fn keep(places: &Places, png: &[u8]) -> Result<PathBuf> {
    let root = root(places);
    let id = format!(
        "{}-{}",
        crate::app_timing::now(),
        crate::text::staged_suffix()
    );
    let folder = root.join(&id);
    let staging = capped_store::stage(&root, folder.clone(), &id)?;
    let file = staging.dir().join(PICTURE);
    std::fs::write(&file, png).context(|| format!("writing {}", file.display()))?;
    staging.place()?;
    Ok(folder.join(PICTURE))
}

/// `builds web picture <url> [--width N] [--full-page]`: waits for this Mac's
/// display, pictures the page, keeps the picture and prints its path.
pub fn cmd(places: &Places, system: &System, args: &Args) -> Result<()> {
    let page = parse(args)?;
    places.require_volume()?;
    let host = Host::local();
    // This Mac's lid holds no step: builds counts this Mac as one that can
    // always draw (lock.rs `Takes`).
    let ready = crate::host_display::hold(&system.ssh(), &host, false, &|_| Ok(()))?;
    if !ready {
        fail!("a signal stopped the wait for this Mac's display");
    }
    let picture = draw(&crate::shots_program::folders(places), &page)?;
    if !picture.settled {
        note(format!(
            "{} had not settled after {} seconds; the picture shows the page as it was then",
            page.url,
            WEB_SETTLE_LIMIT.as_secs()
        ));
    }
    crate::output::answer_line(keep(places, &picture.png)?.display().to_string());
    Ok(())
}

/// The pictures as the cap sees them: one group, a folder for each picture.
fn group(root: &Path) -> Result<Option<Group>> {
    let mut kept = Vec::new();
    for (_, folder) in capped_store::tops_in(root)? {
        let last_use = match capped_store::published_use(root, &folder, PICTURE, "web picture")? {
            capped_store::Published::Used(at) => at,
            capped_store::Published::Gone => continue,
            capped_store::Published::PlanLost => return Ok(None),
        };
        kept.push(Kept::Folder {
            folder: capped_store::Folder::unpinned(folder),
            last_use: Use::At(last_use),
            what: "a web page's picture",
        });
    }
    Ok(Some(Group {
        folder: root.to_path_buf(),
        kept,
    }))
}

/// Deletes the oldest pictures that keep the folder over `CAP_KIB`
/// (prune.rs).
pub fn prune(tool: &Tool, run: Run) -> Result<()> {
    let root = root(&tool.places);
    if !root.is_dir() {
        return Ok(());
    }
    capped_store::prune(tool, run, &root, CAP_KIB, || {
        Ok(group(&root)?.map(|group| vec![group]))
    })
}

#[cfg(test)]
#[path = "web_picture_tests.rs"]
mod tests;
