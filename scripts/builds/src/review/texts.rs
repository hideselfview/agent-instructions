//! The texts of the reviews: what a reviewer was sent (each exchange's system
//! prompt and message), what it answered, and each call's prompt. They are
//! build output, a few hundred KiB for each run, and nothing needs one after
//! a few weeks, so builds keeps them on the build volume under a cap
//! (capped_store.rs) and the store holds only each text's hash: the records of
//! what a review found never depend on a text.
//!
//! A run's texts are one unit, the folder
//! `<volume>/review-texts/<project>/<run id>/`, with one file named for each
//! text's hash, so a text that a run holds several times is kept once. The
//! unit also holds the file `used`, which each read touches, so the cap keeps
//! the runs that agents still read. A run whose texts the cap deleted shows
//! no text: a read says so.

use std::path::PathBuf;

use crate::capped_store::{self, Group, Kept, Use, folders_in};
use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::prune::Run;
use crate::tool::Tool;

/// The folder on the build volume that holds every project's run texts.
pub const FOLDER: &str = "review-texts";

/// How much the folder of the review texts may hold. A run's texts take
/// about 100 KiB to 1 MiB, so the cap holds the runs of weeks of reviews.
pub const CAP_KIB: u64 = 2 * 1024 * 1024;

/// The file of a run's folder that each read touches.
const USED: &str = "used";

/// The hash <text> is kept under: its FNV-1a hash and its length.
pub fn hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}-{}", text.len())
}

/// Where one project's run texts are kept.
#[derive(Clone, Debug)]
pub struct Texts {
    /// The folder of every project's run texts.
    root: PathBuf,
    project: String,
}

/// The folder of the review texts on the volume of <places>.
pub fn root(places: &Places) -> PathBuf {
    places.root.join(FOLDER)
}

impl Texts {
    /// The texts of <project> on the volume of <places>.
    pub fn of(places: &Places, project: &str) -> Texts {
        Texts {
            root: root(places),
            project: project.to_string(),
        }
    }

    fn run_folder(&self, run: i64) -> PathBuf {
        self.root.join(&self.project).join(run.to_string())
    }

    /// Keeps <texts>, each under its hash, as the texts of <run>, in place of
    /// any texts kept for a run of that id.
    pub fn keep_run<'a>(&self, run: i64, texts: impl IntoIterator<Item = &'a str>) -> Result<()> {
        let staging = capped_store::stage(&self.root, self.run_folder(run), &self.project)?;
        for text in texts {
            let file = staging.dir().join(hash(text));
            std::fs::write(&file, text).context(|| format!("writing {}", file.display()))?;
        }
        let used = staging.dir().join(USED);
        std::fs::write(&used, "").context(|| format!("writing {}", used.display()))?;
        staging.place()
    }

    /// The text kept under <hash> for <run>; reading it marks the run used.
    /// Fails when the run has no text under the hash, which is so once the
    /// cap has deleted the run's texts.
    pub fn read(&self, run: i64, hash: &str) -> Result<String> {
        // Held while reading, so that no prune deletes the run meanwhile.
        let _reading = capped_store::writing(&self.root)?;
        let folder = self.run_folder(run);
        let file = folder.join(hash);
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fail!(
                "builds keeps no text {hash} of review run {run}: builds deletes the oldest runs' texts to keep {} under {} (`builds help`, DISK)",
                self.root.display(),
                crate::text::human(CAP_KIB)
            ),
            Err(error) => fail!("reading {}: {error}", file.display()),
        };
        capped_store::mark_used(&folder.join(USED))?;
        Ok(text)
    }
}

/// The review texts as the cap sees them: one group, every project's run
/// folders.
fn group(places: &Places) -> Result<Group> {
    let root = root(places);
    let mut kept = Vec::new();
    for (_, project) in capped_store::tops_in(&root)? {
        for (_, folder) in folders_in(&project)? {
            let used = folder.join(USED);
            let Some(last_use) = capped_store::written_at(&used)? else {
                // Publishing removes the plan marker before changing any
                // files. A valid plan and the exclusive lock rule out a
                // replacement between the missing file and this diagnosis.
                let Some(_valid) = capped_store::lock_plan(&root)? else {
                    return Ok(Group { folder: root, kept });
                };
                if folder
                    .try_exists()
                    .context(|| format!("checking {}", folder.display()))?
                {
                    fail!("The published review run is missing {}.", used.display());
                }
                crate::text::note(format!(
                    "{} disappeared during prune planning",
                    folder.display()
                ));
                continue;
            };
            kept.push(Kept::Folder {
                folder: capped_store::Folder {
                    path: folder,
                    reader_lock: None,
                },
                last_use: Use::At(last_use),
                what: "a run's review texts",
            });
        }
    }
    Ok(Group { folder: root, kept })
}

/// Deletes the oldest runs' texts that keep the folder over `CAP_KIB`
/// (prune.rs).
pub fn prune(tool: &Tool, run: Run) -> Result<()> {
    prune_texts(tool, run, CAP_KIB)
}

/// `capped_store::prune` of the review texts with <cap_kib>.
pub fn prune_texts(tool: &Tool, run: Run, cap_kib: u64) -> Result<()> {
    let root = root(&tool.places);
    if !root.is_dir() {
        return Ok(());
    }
    capped_store::prune(tool, run, &root, cap_kib, || Ok(vec![group(&tool.places)?]))
}

/// Texts of a project in a folder of its own that no other test shares.
#[cfg(test)]
pub fn scratch() -> Texts {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let scratch = crate::testing::Scratch::new(&format!("review-texts-{n}"));
    Texts::of(&scratch.places(), "demo")
}

#[cfg(test)]
#[path = "texts_tests.rs"]
mod tests;
