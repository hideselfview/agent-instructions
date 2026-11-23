//! Keeping Spotlight off each Mac's build root. Spotlight's indexer reads every
//! file a build writes where it indexes, and spends the host's CPU on output
//! that nobody searches.
//!
//! Spotlight skips a folder whose name ends in `.noindex`, so a host's build
//! root in its home folder, `~/builds-root.noindex` (host.rs `REMOTE_ROOT`),
//! needs nothing more. A `.metadata_never_index` file in a folder does not
//! keep Spotlight out on macOS 27: Spotlight indexes the folder's files
//! anyway. A build volume's indexing is a setting of the whole volume, which
//! `mdutil -s` reads and only `sudo mdutil -i off` turns off. `builds host
//! setup` turns it off on the host, this Mac included, when a person is at
//! the terminal to type the password, and `builds status` says whether
//! Spotlight indexes the root. builds never runs mdutil on a root that is a
//! folder, whose volume holds other files.

use crate::fail;
use crate::fail::Result;
use crate::host::{self, Host, Ssh, sh_quote};
use crate::places::Places;

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
    let value = |key: &str| crate::host::answer_value(text, key);
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
/// through sudo (`host::sudo_step`), which fails with no one at this terminal
/// to type the password. A root builds can't keep Spotlight off fails the
/// step. Returns what it says about the host.
pub fn set_up(places: &Places, ssh: &Ssh, host: &Host, at_terminal: bool) -> Result<String> {
    set_up_with(
        &host.name,
        at_terminal,
        read(places, ssh, host)?,
        &mut |script| host::run_at_terminal(ssh, host, script),
    )
}

/// `set_up`, with <root> as the host said it and <run> running a script
/// there at this terminal.
fn set_up_with(
    name: &str,
    at_terminal: bool,
    root: Root,
    run: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    let place = host::place(name);
    match root {
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
            host::sudo_step(
                name,
                at_terminal,
                &format!(
                    "Spotlight indexes the build volume {root} on {place} (mdutil -s says: {said}), so its indexer reads every file a build writes there"
                ),
                &[command(&root)],
                run,
            )?;
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
