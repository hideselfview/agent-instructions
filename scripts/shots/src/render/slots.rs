//! The limit on pages drawing at once on one host. Every program that starts the
//! pinned Chrome (a shots run, `builds shots draws`, a test) takes a slot for
//! each page it has open and gives it back when the page closes, so the pages
//! drawing on the host never pass the limit, however many processes draw.
//!
//! A slot is an exclusive flock on one of `count` files in a folder on the
//! host. The kernel gives the lock back when the process ends, however it ends,
//! so a process that dies frees its slots.

use std::fs::File;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// The slots a host of <cores> cores offers: half of them, and at least one. A
/// page draws on several threads (the compositor, the renderer and the browser
/// process, besides the one raster thread), so half the cores leaves room for
/// the host's other work.
fn chrome_slots(cores: NonZeroUsize) -> NonZeroUsize {
    NonZeroUsize::new(cores.get() / 2).unwrap_or(NonZeroUsize::MIN)
}

/// The slots one host offers for pages.
#[derive(Clone, Debug)]
pub(crate) struct Slots {
    folder: PathBuf,
    count: usize,
}

/// A page's place among the host's drawing pages, given back on drop.
#[derive(Debug)]
pub(crate) struct Slot {
    _lock: File,
}

impl Slots {
    /// The host's slots in the build volume <volume>: half the host's cores
    /// (`chrome_slots`). Fails when the host does not say how many cores it
    /// has.
    pub(crate) fn of_host(volume: &Path) -> Result<Self, String> {
        let cores = std::thread::available_parallelism()
            .map_err(|e| format!("cannot read the host's core count: {e}"))?;
        Ok(Self::new(
            &volume.join(crate::CHROME_SLOTS_FOLDER),
            chrome_slots(cores),
        ))
    }

    /// <count> slots in <folder>.
    pub(crate) fn new(folder: &Path, count: NonZeroUsize) -> Self {
        Self {
            folder: folder.to_path_buf(),
            count: count.get(),
        }
    }

    /// Takes a slot, waiting while every slot is held. Notes once on stderr
    /// that it waits, naming <what> it waits to draw.
    pub(crate) async fn take(&self, what: &str) -> Result<Slot, String> {
        let slots = self.clone();
        let what = what.to_string();
        tokio::task::spawn_blocking(move || slots.take_waiting(&what))
            .await
            .map_err(|e| format!("taking a Chrome slot failed: {e}"))?
    }

    fn take_waiting(&self, what: &str) -> Result<Slot, String> {
        let fail = |e: std::io::Error| {
            format!(
                "cannot take a Chrome slot in {}: {e}",
                self.folder.display()
            )
        };
        // Makes only the slots' folder, never the build volume's, so a volume
        // that is not mounted fails the slot and leaves nothing on the system
        // disk.
        match std::fs::create_dir(&self.folder) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(fail(e)),
            _ => {}
        }
        let files = (0..self.count)
            .map(|index| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(self.folder.join(format!("slot-{index}")))
                    .map_err(fail)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for file in &files {
            match file.try_lock() {
                Ok(()) => {
                    return Ok(Slot {
                        _lock: file.try_clone().map_err(fail)?,
                    });
                }
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(std::fs::TryLockError::Error(e)) => return Err(fail(e)),
            }
        }
        crate::log::note(format_args!(
            "waiting for one of the host's {} Chrome slots, to draw {what}",
            self.count
        ));
        // Wait on a slot picked by the process and the time, so that waiters
        // spread over the slots; the kernel wakes this one when its holder
        // gives the slot back.
        let pick = (std::process::id() as usize).wrapping_add(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos() as usize),
        ) % self.count;
        let file = &files[pick];
        file.lock().map_err(fail)?;
        Ok(Slot {
            _lock: file.try_clone().map_err(fail)?,
        })
    }
}

#[cfg(test)]
#[path = "slots_tests.rs"]
mod tests;
