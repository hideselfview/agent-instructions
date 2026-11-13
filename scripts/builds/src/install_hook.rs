//! `builds install-hook <repo>`: writes the repo's pre-commit hook, a script
//! that runs `builds checks run` (checks_run.rs). It goes where git looks for
//! hooks: `core.hooksPath` when set, else the hooks folder of the git common
//! dir, which every worktree shares. Running it again overwrites the same
//! file.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::checkout::{git, git_text};
use crate::fail;
use crate::fail::{Context, Result};
use crate::text::note;

const HOOK: &str = "#!/bin/sh
# Written by `builds install-hook`: runs the repo's .checks on the staged files.
command -v builds >/dev/null 2>&1 || {
    echo \"builds is not on PATH; the pre-commit hook needs it\" >&2
    exit 1
}
exec builds checks run
";

pub fn cmd_install_hook(args: &[String]) -> Result<()> {
    let [repo] = args else { fail!("usage: builds install-hook <repo>") };
    let path = install(Path::new(repo))?;
    note(format!("pre-commit hook written to {}", path.display()));
    Ok(())
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
    std::fs::write(&path, HOOK).context(|| format!("writing {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .context(|| format!("making {} executable", path.display()))?;
    Ok(path)
}

#[cfg(test)]
#[path = "install_hook_tests.rs"]
mod tests;
