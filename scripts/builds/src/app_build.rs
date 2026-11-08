//! `builds app-build <repo> <ref> [<step>...]`: on the repo's app lock, checks
//! <ref> out in the app checkout (which must be clean) and runs the steps its
//! `.app-build` names, each through `run_in_group`, in the order given
//! (every step, in the order written, when none is named), stopping at the
//! first one that fails. `builds app-build <repo> --list` prints the steps.
//!
//! A takeover stops the step running; the build then runs again from the
//! checkout, behind the build that took over.

use std::path::Path;

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lock::{self, Ran};
use crate::places::Places;
use crate::app_config::{CONFIG, Config, Step, app_checkout_top, check_out, parse_config, run_command};
use crate::text::note;

const USAGE: &str = "usage: builds app-build <repo> <ref> [<step>...] | builds app-build <repo> --list";

/// The steps to run: those named in order, or all of <config>'s.
fn chosen(config: &Config, names: &[String]) -> Result<Vec<Step>> {
    if names.is_empty() {
        return Ok(config.steps.clone());
    }
    names
        .iter()
        .map(|name| match config.steps.iter().find(|step| step.name == *name) {
            Some(step) => Ok(step.clone()),
            None => {
                let known: Vec<&str> = config.steps.iter().map(|step| step.name.as_str()).collect();
                fail!("{CONFIG} has no step {name} (it has: {})", known.join(", "))
            }
        })
        .collect()
}

/// Runs <steps> in the checkout <top> in order, stopping at the first that
/// fails; `TakenOver` when a takeover stopped one.
fn run_steps(top: &Path, steps: &[Step]) -> Result<Ran> {
    for step in steps {
        note(format!("step {}", step.name));
        match run_command(top, &step.command) {
            Ok(Ran::Finished) => {}
            Ok(Ran::TakenOver) => return Ok(Ran::TakenOver),
            Err(Fail::Exit(code)) => {
                note(format!("step {} failed (exit {code})", step.name));
                return Err(Fail::Exit(code));
            }
            Err(other) => return Err(other),
        }
    }
    Ok(Ran::Finished)
}

pub fn cmd_app_build(places: &Places, args: &[String]) -> Result<()> {
    let [repo, rest @ ..] = args else {
        fail!("{USAGE}");
    };
    let top = app_checkout_top(places, repo)?;
    if let [flag] = rest
        && flag == "--list"
    {
        let file = top.join(CONFIG);
        let text = std::fs::read_to_string(&file).context(|| format!("reading {}", file.display()))?;
        for step in parse_config(&text)?.steps {
            println!("{} {}", step.name, step.command.join(" "));
        }
        return Ok(());
    }
    let [commit, names @ ..] = rest else {
        fail!("{USAGE}");
    };
    if commit.starts_with('-') {
        fail!("{USAGE}");
    }
    note(format!("waiting for {repo}'s app lock, then building {commit}"));
    let mut held = lock::take(places, repo)?;
    loop {
        let config = check_out(&top, commit)?;
        let steps = chosen(&config, names)?;
        if steps.is_empty() {
            fail!("{CONFIG} names no step");
        }
        match run_steps(&top, &steps)? {
            Ran::Finished => return Ok(()),
            Ran::TakenOver => {
                note(format!("the build was stopped for a takeover; building {commit} again after it"));
                held = held.requeue()?;
            }
        }
    }
}

#[cfg(test)]
#[path = "app_build_tests.rs"]
mod tests;
