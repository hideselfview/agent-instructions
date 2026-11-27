//! `builds-rustc`: the compiler wrapper `builds cargo` gives cargo
//! (`cargo_env`). builds-rustc runs sccache with rustc's arguments, and for a
//! crate whose build script wrote files into its OUT_DIR it adds
//! `--remap-path-prefix=<OUT_DIR>=/OUT_DIR`.
//!
//! cargo puts a build script's OUT_DIR inside the checkout's target, so its
//! path names the checkout. A crate that includes a file its build script
//! wrote there, as serde and thiserror include `private.rs`, records that
//! path in its rmeta and rlib, so without the remap the same crate built in
//! two checkouts differs in those bytes. sccache hashes the bytes of every
//! library a crate names with `--extern`, so every crate above serde or
//! thiserror would miss the cache in each new checkout. With the remap the
//! output names `/OUT_DIR/...` wherever it was built, and the crates above it
//! hit the cache across checkouts.
//!
//! The added argument names the checkout, and sccache hashes it, so a crate
//! that gets it misses the cache in each new checkout. Such a crate already
//! did when it reads its OUT_DIR, since sccache hashes the environment
//! values rustc reports reading. A crate whose build script wrote nothing
//! there gets no argument, and so keeps its hits.
//!
//! `~/.local/bin/builds-rustc` is a symlink to builds (`builds install`,
//! `builds host setup`), and builds runs as the wrapper when started by that
//! name.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};

/// The name builds runs as the compiler wrapper under.
pub const NAME: &str = "builds-rustc";

/// The compiler cache the wrapper runs, found on PATH.
const SCCACHE: &str = "sccache";

/// The path an OUT_DIR becomes in what rustc writes.
const REMAPPED: &str = "/OUT_DIR";

/// The `RUSTC_WRAPPER` that `builds cargo` gives cargo when the caller's,
/// <caller>, is unset or sccache itself: this wrapper. None leaves the
/// caller's own wrapper in place, such as bae's clippy wrapper, which runs
/// this wrapper itself where it would run sccache. cargo reads the variable
/// before its config files, so the wrapper also replaces a repo's
/// `build.rustc-wrapper = "sccache"`.
pub fn cargo_env(caller: Option<&OsStr>, home: &Path) -> Option<PathBuf> {
    match caller {
        None => Some(crate::links::path(home, NAME)),
        Some(caller) if caller == OsStr::new(SCCACHE) => Some(crate::links::path(home, NAME)),
        Some(_) => None,
    }
}

/// The argument to add to rustc's for a crate whose OUT_DIR is <out_dir>
/// (None when it has no build script): the remap of that folder, when the
/// build script wrote anything there.
pub fn added_arg(out_dir: Option<&Path>) -> Result<Option<OsString>> {
    let Some(out_dir) = out_dir else {
        return Ok(None);
    };
    let mut entries =
        std::fs::read_dir(out_dir).context(|| format!("reading {}", out_dir.display()))?;
    if entries.next().is_none() {
        return Ok(None);
    }
    let mut remap = OsString::from("--remap-path-prefix=");
    remap.push(out_dir);
    remap.push("=");
    remap.push(REMAPPED);
    Ok(Some(remap))
}

/// Runs sccache with <args> (the compiler, then its arguments) and what
/// `added_arg` adds for the OUT_DIR in this process's environment, in this
/// process. `run` returns only when it cannot start sccache.
pub fn run(args: &[OsString]) -> Result<std::convert::Infallible> {
    use std::os::unix::process::CommandExt;
    let out_dir = std::env::var_os("OUT_DIR").map(PathBuf::from);
    let added = added_arg(out_dir.as_deref())?;
    crate::output::finish();
    let error = std::process::Command::new(SCCACHE)
        .args(args)
        .args(added)
        .exec();
    fail!("running {SCCACHE}: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn only_an_out_dir_with_files_gets_the_remap() {
        let scratch = Scratch::new("rustc-wrapper-remap");
        let out = scratch.dir.join("target/debug/build/serde/0d7b/out");
        std::fs::create_dir_all(&out).unwrap();
        assert_eq!(added_arg(None).unwrap(), None);
        assert_eq!(added_arg(Some(&out)).unwrap(), None, "an empty OUT_DIR");
        std::fs::write(out.join("private.rs"), "").unwrap();
        assert_eq!(
            added_arg(Some(&out)).unwrap(),
            Some(OsString::from(format!(
                "--remap-path-prefix={}=/OUT_DIR",
                out.display()
            )))
        );
        assert!(added_arg(Some(&scratch.dir.join("gone"))).is_err());
    }

    #[test]
    fn the_wrapper_replaces_only_an_unset_wrapper_or_sccache() {
        let home = Path::new("/Users/someone");
        let wrapper = PathBuf::from("/Users/someone/.local/bin/builds-rustc");
        assert_eq!(cargo_env(None, home), Some(wrapper.clone()));
        assert_eq!(cargo_env(Some(OsStr::new("sccache")), home), Some(wrapper));
        assert_eq!(
            cargo_env(
                Some(OsStr::new("/d/bae/scripts/clippy-rustc-wrapper.sh")),
                home
            ),
            None
        );
        assert_eq!(cargo_env(Some(OsStr::new("")), home), None);
    }
}
