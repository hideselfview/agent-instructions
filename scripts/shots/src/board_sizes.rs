//! Measuring the boards of the component scenes a capture takes, before the
//! capture, so the harness can draw each component at its board's size.

use crate::Checker;
use crate::design::DesignSystem;
use crate::names::{Name, Previews};
use crate::render::serve::Tree;
use crate::render::{self, Scales, runtime};
use crate::settings::Settings;
use crate::shots::{ElementList, Size};
use std::collections::BTreeMap;
use std::path::Path;

/// The size in points of the board of each component scene in `names`, drawn
/// by the renderer that draws the boards for the comparison. A size does not
/// depend on the scale, so the boards are drawn at 1. Screen scenes have no
/// entry, and no browser starts when `names` holds no component scene.
pub(crate) fn measure(
    root: &Path,
    checker: &Checker,
    settings: &Settings,
    names: &[Name],
) -> Result<BTreeMap<Name, Size>, String> {
    let components: Vec<Name> = names
        .iter()
        .filter(|n| matches!(n, Name::Component(_)))
        .cloned()
        .collect();
    if components.is_empty() {
        return Ok(BTreeMap::new());
    }
    let runtime = runtime::check(root).map_err(|e| e.to_string())?;
    let design = DesignSystem::of(root, checker.folders).map_err(|e| e.to_string())?;
    let previews = Previews::of(root)?;
    let tree = Tree {
        root,
        design: &design,
        runtime: runtime.copy.text,
    };
    let scales: Scales = components.iter().map(|n| (n.clone(), 1.0)).collect();
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    render::boards(
        tree,
        settings,
        &previews,
        checker.folders,
        &components,
        &scales,
        scratch.path(),
    )?;
    components
        .into_iter()
        .map(|name| {
            let list = ElementList::load(&scratch.path().join(format!("{name}.json")))
                .map_err(|e| e.to_string())?;
            Ok((name, list.size))
        })
        .collect()
}
