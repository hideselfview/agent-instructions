//! `builds install-hook <repo>`: writes the repo's pre-commit hook, a script
//! that runs `builds checks run` (checks_run.rs). It goes where git looks for
//! hooks: `core.hooksPath` when set, else the hooks folder of the git common
//! dir, which every worktree shares. Running it again overwrites the same
//! file.
//!
//! Every other executable file in the repo's `scripts/hooks` gets a forwarder
//! of its own name (a fresh clone needs `post-checkout`, which sets a worktree
//! up). A forwarder runs the copy in the checkout git is working in, so a
//! worktree runs its own branch's hook rather than the installing checkout's.
//! A name with a dot is a helper script, not a git hook, and gets none.

use std::path::{Path, PathBuf};

use crate::fail;
use crate::fail::{Context, Result};
use crate::output::note;
use boards::{git, git_text};

const HOOK: &str = "#!/bin/sh
# Written by `builds install-hook`: runs the repo's .checks on the staged files.
command -v builds >/dev/null 2>&1 || {
    echo \"builds is not on PATH; the pre-commit hook needs it\" >&2
    exit 1
}
exec builds checks run
";

pub fn cmd_install_hook(args: &[String]) -> Result<()> {
    let [repo] = args else {
        fail!("usage: builds install-hook <repo>")
    };
    let path = install(Path::new(repo))?;
    note(format!("pre-commit hook written to {}", path.display()));
    Ok(())
}

fn forwarder(name: &str) -> String {
    format!(
        "#!/bin/sh
# Written by `builds install-hook`: runs this checkout's own {name}.
hook=\"$(git rev-parse --show-toplevel)/scripts/hooks/{name}\"
[ -x \"$hook\" ] || exit 0
exec \"$hook\" \"$@\"
"
    )
}

/// The names of the executable files in <top>/scripts/hooks that are git
/// hooks other than pre-commit, sorted.
fn forwarded_hooks(top: &Path) -> Result<Vec<String>> {
    let dir = top.join("scripts/hooks");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("reading {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if crate::worktree::is_executable(&entry.path())
            && name != "pre-commit"
            && !name.contains('.')
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

pub fn install(repo: &Path) -> Result<PathBuf> {
    let dir = git_text(
        git(repo).args(["rev-parse", "--path-format=absolute", "--git-path", "hooks"]),
        &format!("find the hooks folder of {}", repo.display()),
    )?;
    let dir = PathBuf::from(dir.trim());
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    let dir = std::fs::canonicalize(&dir).context(|| format!("resolving {}", dir.display()))?;
    let path = dir.join("pre-commit");
    crate::whole_file::write_executable(&path, HOOK)?;
    let top = git_text(
        git(repo).args(["rev-parse", "--show-toplevel"]),
        "find the top of the repo",
    )?;
    for name in forwarded_hooks(Path::new(top.trim()))? {
        crate::whole_file::write_executable(&dir.join(&name), forwarder(&name))?;
    }
    Ok(path)
}

#[cfg(test)]
#[path = "install_hook_tests.rs"]
mod tests;
