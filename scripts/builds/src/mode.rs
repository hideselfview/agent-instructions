//! `builds mode [quick|strict]`: the machine-wide mode the pre-commit hooks
//! run in. It is one file on the volume, <root>/mode; absent means strict.

use crate::fail;
use crate::fail::{Context, Result};
use crate::places::Places;
use crate::text::note;
use crate::tool::Tool;

const MODES: [&str; 2] = ["quick", "strict"];

fn mode_file(places: &Places) -> std::path::PathBuf {
    places.root.join("mode")
}

/// The recorded mode; strict when none is.
pub fn current(places: &Places) -> Result<&'static str> {
    let file = mode_file(places);
    match std::fs::read_to_string(&file) {
        Ok(text) => match MODES.into_iter().find(|mode| *mode == text.trim_end_matches('\n')) {
            Some(mode) => Ok(mode),
            None => fail!("{} holds '{}', not quick or strict", file.display(), text.trim_end()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("strict"),
        Err(error) => fail!("reading {}: {error}", file.display()),
    }
}

pub fn cmd_mode(tool: &Tool, args: &[String]) -> Result<()> {
    match args {
        [] => {
            println!("{}", current(&tool.places)?);
            Ok(())
        }
        [mode] if MODES.contains(&mode.as_str()) => {
            tool.places.require_volume()?;
            let file = mode_file(&tool.places);
            std::fs::write(&file, format!("{mode}\n")).context(|| format!("writing {}", file.display()))?;
            note(format!("pre-commit hooks run in {mode} mode ({})", file.display()));
            Ok(())
        }
        _ => fail!("usage: builds mode [quick|strict]"),
    }
}

#[cfg(test)]
#[path = "mode_tests.rs"]
mod tests;
