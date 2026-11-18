//! Keeping Spotlight off each Mac's build root. Spotlight's indexer reads every
//! file a build writes on a volume it indexes: on 2026-11-17 the index of m1's
//! Builds volume held 187,078 files, 18,538 of them rlibs and 24,518 object
//! files, and m1's mds_stores had used 2h48m of CPU in three days.
//!
//! Spotlight skips a folder whose name ends in `.noindex`, so a host's build
//! root in its home folder, `~/builds-root.noindex` (host.rs `REMOTE_ROOT`),
//! needs nothing more. A `.metadata_never_index` file in a folder does not
//! keep Spotlight out: on m1 (macOS 27), Spotlight indexed the files of a
//! folder that held one within seconds. A build volume's indexing is a
//! setting of the whole volume, which `mdutil -s` reads and only
//! `sudo mdutil -i off` turns off. `builds host setup` turns it off on the
//! host, this Mac included, when a person is at the terminal to type the
//! password, and `builds status` says whether Spotlight indexes the root.
//! builds never runs mdutil on a root that is a folder, whose volume holds
//! other files.

use crate::fail;
use crate::fail::{Fail, Result};
use crate::host::{self, Host, Ssh, sh_quote};
use crate::places::Places;
use crate::text::note;

/// A Mac's build root, as Spotlight treats it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Root {
    /// A folder whose name, or the name of a folder it is in, ends in
    /// `.noindex`: Spotlight skips it.
    Named { root: String },
    /// A volume of its own; <said> is what `mdutil -s` says of its indexing.
    Volume { root: String, said: String },
    /// A folder with no `.noindex` name, on a volume that holds other files.
    Folder { root: String },
}

impl Root {
    /// Does Spotlight keep out of the root?
    pub fn kept_out(&self) -> bool {
        match self {
            Root::Named { .. } => true,
            Root::Volume { said, .. } => indexing_off(said),
            Root::Folder { .. } => false,
        }
    }
}

/// Does <said>, what `mdutil -s` says of a volume, say its indexing is off?
/// mdutil says "Indexing disabled." or "Indexing and searching disabled."
/// when it is off, "Indexing enabled." when it is on, and an error when it
/// can't tell.
fn indexing_off(said: &str) -> bool {
    said.contains("disabled")
}

/// The command that turns off Spotlight's indexing of the build volume
/// <root>.
fn command(root: &str) -> String {
    format!("sudo mdutil -i off {}", sh_quote(root))
}

/// Prints what `parse` reads about the build root in BUILDS_ROOT: its path,
/// whether it is a volume of its own (its device differs from its parent's),
/// and for a volume, what `mdutil -s` says without the line that names the
/// volume. Each tool only reads.
const SCRIPT: &str = r#"root="$BUILDS_ROOT"
echo "spotlight_root=$root"
if [ ! -d "$root" ]; then echo spotlight_kind=missing
elif [ "$(stat -f %d "$root")" != "$(stat -f %d "$root/..")" ]; then
  echo spotlight_kind=volume
  echo "spotlight_said=$(/usr/bin/mdutil -s "$root" 2>&1 | sed '/:$/d' | tr '\n\t' '  ')"
else echo spotlight_kind=folder
fi"#;

/// Reads what `SCRIPT` printed.
pub fn parse(text: &str) -> Result<Root> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
    let root = match value("spotlight_root") {
        Some(root) if !root.is_empty() => root.to_string(),
        _ => fail!("the answer about the build root names no root: {text:?}"),
    };
    match value("spotlight_kind") {
        Some("volume") => match value("spotlight_said") {
            Some(said) => Ok(Root::Volume {
                root,
                said: said.to_string(),
            }),
            None => fail!("the answer about the build volume {root} has no mdutil line: {text:?}"),
        },
        Some("folder") if root.split('/').any(|name| name.ends_with(".noindex")) => {
            Ok(Root::Named { root })
        }
        Some("folder") => Ok(Root::Folder { root }),
        Some("missing") => fail!("the build root {root} is not there"),
        _ => fail!("the answer about the build root {root} does not say what it is: {text:?}"),
    }
}

/// Asks <host> about its build root: the root `remote::root_script` picks
/// there, or <places>' root on this Mac.
pub fn read(places: &Places, ssh: &Ssh, host: &Host) -> Result<Root> {
    let root = match host.target {
        Some(_) => crate::remote::root_script(host),
        None => format!(
            "BUILDS_ROOT={}\n",
            sh_quote(&places.root.display().to_string())
        ),
    };
    let transport = crate::remote::Transport { places, ssh, host };
    parse(&transport.capture(
        &format!("{root}{SCRIPT}"),
        "checking whether Spotlight indexes the build root",
    )?)
}

/// `builds host setup`'s step for Spotlight: checks <host>'s build root, and
/// when Spotlight indexes its build volume, turns the volume's indexing off
/// through sudo if a person is at this terminal (<at_terminal>) to type the
/// password, then checks again. With no one at the terminal, with the
/// volume still indexed, or with a root builds can't keep Spotlight off, it
/// fails, naming what to do. Returns what it says about the host.
pub fn set_up(places: &Places, ssh: &Ssh, host: &Host, at_terminal: bool) -> Result<String> {
    set_up_with(
        &host.name,
        at_terminal,
        &mut || read(places, ssh, host),
        &mut |script| host::run_at_terminal(ssh, host, script),
    )
}

/// `set_up`, with <read> asking the host about its build root and <run>
/// running a script there at this terminal.
fn set_up_with(
    name: &str,
    at_terminal: bool,
    read: &mut dyn FnMut() -> Result<Root>,
    run: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    let place = host::place(name);
    let again = format!("Then `builds host setup {name}` checks it again");
    match read()? {
        Root::Named { root } => Ok(format!(
            "Spotlight skips its build root {root}, whose name ends in .noindex"
        )),
        Root::Folder { root } => fail!(
            "Spotlight may index the build root {root} on {place}: the root is a folder whose name does not end in .noindex, on a volume that holds other files. builds keeps Spotlight off a build volume through mdutil, and off a build root in a home folder by its name, ~/{}",
            host::REMOTE_ROOT
        ),
        Root::Volume { root, said } if indexing_off(&said) => Ok(format!(
            "Spotlight indexing is off on its build volume {root}"
        )),
        Root::Volume { root, said } => {
            let command = command(&root);
            if !at_terminal {
                fail!(
                    "Spotlight indexes the build volume {root} on {place} (mdutil -s says: {said}), so its indexer reads every file a build writes there, and no one is at this terminal to type the sudo password. Run `{command}` on {place}. {again}"
                );
            }
            note(format!(
                "turning off Spotlight indexing on the build volume {root} on {place} through sudo, which asks for the password there: {command}"
            ));
            let ran = run(&command);
            let after = read()?;
            if !after.kept_out() {
                let why = match ran {
                    Err(Fail::Message(message)) => format!(" ({message})"),
                    _ => String::new(),
                };
                let said = match &after {
                    Root::Volume { said, .. } => said.as_str(),
                    _ => "",
                };
                fail!(
                    "Spotlight still indexes the build volume {root} on {place} (mdutil -s says: {said}){why}. Run `{command}` on {place}. {again}"
                );
            }
            Ok(format!(
                "turned off Spotlight indexing on its build volume {root}"
            ))
        }
    }
}

/// The line `builds status` prints about this Mac's build root, from
/// <read>; <host_mac> says whether this Mac is a host
/// (`Places::is_host`), whose setup another Mac runs.
pub fn status_line(read: Result<Root>, host_mac: bool) -> String {
    let root = match read {
        Ok(root) => root,
        Err(fail) => {
            return format!(
                "spotlight: builds could not read whether Spotlight indexes the build root: {}",
                fail.text()
            );
        }
    };
    match root {
        Root::Named { root } => {
            format!("spotlight: skips the build root {root}, whose name ends in .noindex")
        }
        Root::Volume { root, said } if indexing_off(&said) => {
            format!("spotlight: indexing is off on the build volume {root}")
        }
        Root::Volume { root, said } => {
            let fix = if host_mac {
                "Turn it off from a terminal on the Mac that sends jobs here, with `builds host setup <this host's name>`"
            } else {
                "Turn it off from a terminal with `builds host setup local`"
            };
            format!(
                "spotlight: INDEXES the build volume {root} (mdutil -s says: {said}), so its indexer reads every file a build writes here. {fix}"
            )
        }
        Root::Folder { root } => format!(
            "spotlight: may index the build root {root}, a folder whose name does not end in .noindex, on a volume that holds other files"
        ),
    }
}

#[cfg(test)]
#[path = "spotlight_tests.rs"]
mod tests;
