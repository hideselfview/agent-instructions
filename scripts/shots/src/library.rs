//! What builds asks the checker outside a run: facts read from a repo's files
//! that no shot, board picture or finding depends on, so this file is in no
//! version of the checker (builds' build_source.rs).

use std::path::Path;

use crate::declared::{self, Declared};
use crate::names::App;
use crate::settings::Settings;

/// The CSS selector of a component preview row's state name, from the
/// `shots-preview` line in the shots checker's settings, if they have one.
pub fn preview_name_selector(config: &str) -> Result<Option<String>, String> {
    let settings = Settings::parse(config).map_err(|e| e.to_string())?;
    Ok(settings.preview.map(|preview| preview.name))
}

/// Each shot the harnesses in <folders> declare under <root>, in name order.
pub fn declared_scenes(root: &Path, folders: &[(App, String)]) -> Result<Vec<Declared>, String> {
    declared::read(root, folders).map_err(|e| e.to_string())
}
