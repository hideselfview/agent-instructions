//! Every change to a build root, `<volume>/<repo>/<checkout>` (checkout.rs
//! `Root`): making one, writing in it (a build, a clone, a change of its
//! checkout or lane), deleting a folder inside it and removing it. The
//! dashboard watches nothing on the build volume (sizes.rs), so it learns of
//! a change only from the command that made it. Each change adds its root to
//! the command's `Changed`, and when the command ends, main measures each
//! root in it once and tells the dashboard of them all (`Changed::record`),
//! whether the command passed or failed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::checkout::{Checkout, Root};
use crate::evict::{Recreate, Removal, evict};
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::tool::Tool;

/// The build roots one command changed. A clone adds to the same set: main
/// makes it, the command's `Tool` carries a clone, and `builds cargo` and
/// `builds lake`, which run without a `Tool`, add their checkout's root to
/// main's.
#[derive(Clone, Debug, Default)]
pub struct Changed(Arc<Mutex<BTreeSet<PathBuf>>>);

impl Changed {
    /// The command writes in the build root <root>, or changes the checkout
    /// or the lane the root belongs to.
    pub fn add(&self, root: PathBuf) {
        self.0.lock().expect("no panic holding it").insert(root);
    }

    /// The command changes a folder at or inside <path>: adds the build root
    /// that holds <path>, when one does (`Root::holding`).
    pub fn add_holding(&self, path: &Path) -> Result<()> {
        if let Some(root) = Root::holding(path)? {
            self.add(root);
        }
        Ok(())
    }

    /// The roots added so far, sorted.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.0
            .lock()
            .expect("no panic holding it")
            .iter()
            .cloned()
            .collect()
    }

    /// Measures each root added, writes its record and tells the dashboard
    /// of them all (`sizes::record_roots`).
    pub fn record(&self, places: &Places) {
        crate::sizes::record_roots(places, &self.roots());
    }
}

/// Makes <checkout>'s build root, with the marker that names the checkout,
/// and returns the root's path.
pub fn make(tool: &Tool, checkout: &Checkout) -> Result<PathBuf> {
    let root = checkout.build_root(&tool.places);
    tool.changed.add(root.clone());
    std::fs::create_dir_all(&root).context(|| format!("creating {}", root.display()))?;
    Root::mark(&root, &checkout.top)?;
    Ok(root)
}

/// Deletes <dir> whole unless a build uses it (`evict::evict`), and adds the
/// build root that holds it: the root <dir> is in, or <dir> itself when it
/// is a root, found from <dir> as the caller names it, as the build roots'
/// paths join the volume's. Returns false, deleting nothing, when <dir> is
/// in use.
pub fn delete(tool: &Tool, dir: &Path, recreate: Recreate, removal: Removal) -> Result<bool> {
    tool.changed.add_holding(dir)?;
    evict(&tool.places, dir, recreate, removal)
}

#[cfg(test)]
#[path = "roots_tests.rs"]
mod tests;
