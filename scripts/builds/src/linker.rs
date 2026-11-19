//! `builds-linker`: the linker `builds cargo` gives rustc for this Mac's own
//! target (`cargo_config`). builds-linker runs `cc`, rustc's default linker
//! here, with rustc's arguments, and gives each proc-macro dylib the install
//! name `@rpath/<its file name>`.
//!
//! ld names a dylib by its output path unless told otherwise, so a
//! proc-macro built in two checkouts differs in that name, and so in its
//! LC_UUID and signature, though nothing else differs. sccache hashes each
//! proc-macro a crate depends on, so sccache never reused another checkout's
//! output for a crate that depends on a proc-macro. rustc loads a proc-macro
//! by its path and never reads its install name. Any other dylib keeps the
//! name ld gives it: the dynamic loader finds a cdylib, such as bae-bridge's,
//! by its install name.
//!
//! `~/.local/bin/builds-linker` is a symlink to builds (`builds install`,
//! `builds host setup`), and builds runs as the linker when started by that
//! name.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};

/// The name builds runs as the linker under.
pub const NAME: &str = "builds-linker";

/// The symbol rustc exports from a proc-macro dylib, and from no other kind.
const PROC_MACRO_SYMBOL: &str = "_rustc_proc_macro_decls_";

/// The `--config` arguments that make cargo link this Mac's own target with
/// `builds-linker`. Every checkout gets the same path, so the setting gives
/// every checkout the same sccache keys.
pub fn cargo_config(home: &Path) -> [String; 2] {
    [
        "--config".to_string(),
        format!(
            "target.{}-apple-darwin.linker={:?}",
            std::env::consts::ARCH,
            crate::links::path(home, NAME).display().to_string()
        ),
    ]
}

/// The file rustc lists a link's exported symbols in, from its arguments:
/// `-Wl,-exported_symbols_list` then `-Wl,<file>`, or both in one argument.
fn exported_symbols_list(args: &[OsString]) -> Option<PathBuf> {
    let words: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    words.iter().enumerate().find_map(|(at, word)| {
        if let Some(file) = word.strip_prefix("-Wl,-exported_symbols_list,") {
            return Some(PathBuf::from(file));
        }
        if word == "-Wl,-exported_symbols_list" {
            return words
                .get(at + 1)
                .and_then(|next| next.strip_prefix("-Wl,"))
                .map(PathBuf::from);
        }
        None
    })
}

/// The arguments to add to rustc's <args> for `cc`: the install name
/// `@rpath/<output's file name>` when the link makes a proc-macro dylib (its
/// exported symbols include rustc's proc-macro symbol) and names none of its
/// own, else nothing.
pub fn added_args(args: &[OsString]) -> Result<Vec<OsString>> {
    if !args.iter().any(|arg| arg == "-dynamiclib")
        || args
            .iter()
            .any(|arg| arg.to_string_lossy().contains("-install_name"))
    {
        return Ok(Vec::new());
    }
    let Some(list) = exported_symbols_list(args) else {
        return Ok(Vec::new());
    };
    let symbols =
        std::fs::read_to_string(&list).context(|| format!("reading {}", list.display()))?;
    if !symbols.contains(PROC_MACRO_SYMBOL) {
        return Ok(Vec::new());
    }
    let Some(output) = args
        .iter()
        .position(|arg| arg == "-o")
        .and_then(|at| args.get(at + 1))
    else {
        fail!("rustc linked a proc-macro without naming its output (-o)");
    };
    let Some(name) = Path::new(output).file_name() else {
        fail!("the proc-macro's output {output:?} has no file name");
    };
    let mut install_name = OsString::from("-Wl,-install_name,@rpath/");
    install_name.push(name);
    Ok(vec![install_name])
}

/// Runs `cc` with <args> and what `added_args` adds, in this process. `run`
/// returns only when it cannot start cc.
pub fn run(args: &[OsString]) -> Result<std::convert::Infallible> {
    use std::os::unix::process::CommandExt;
    let added = added_args(args)?;
    let error = std::process::Command::new("cc")
        .args(args)
        .args(added)
        .exec();
    fail!("running cc: {error}")
}

#[cfg(test)]
#[path = "linker_tests.rs"]
mod tests;
