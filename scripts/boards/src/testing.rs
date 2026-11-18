//! What the tests build: scratch folders, git repos, and a design system as
//! a repo holds one.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A folder of its own for one test, emptied when the test starts.
pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        let base = std::fs::canonicalize(std::env::temp_dir()).expect("the temp dir resolves");
        let dir = base.join(format!("boards-test-{name}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("an old scratch folder is removable");
        }
        std::fs::create_dir_all(&dir).expect("the scratch folder is creatable");
        Scratch { dir }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// What `git <args>` prints in <dir>, trimmed; fails the test when git fails.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A new repo at <dir> with one commit.
pub fn repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("creatable");
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "test"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "Start"]);
}

/// Commits everything under `design` in <dir> that git does not ignore and
/// returns the commit's full id.
pub fn commit(dir: &Path) -> String {
    git(dir, &["add", "design"]);
    git(dir, &["commit", "-q", "-m", "design"]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A tokens.json whose stylesheet holds one property, `--tokens: <value>;`.
pub fn tokens_json(value: &str) -> String {
    serde_json::json!({
        "name": "test",
        "color": { "themes": [{ "id": "dark" }], "tokens": [] },
        "type": { "families": {}, "groups": [] },
        "spacing": { "tokens": [{ "name": "tokens", "value": value }] },
    })
    .to_string()
}

/// A design system in the worktree at <dir> as a repo holds one: its
/// sources (tokens.json, whose stylesheet holds `--tokens: <token>;`, and
/// components/src/bundle.css and bundle.js) and the build-bundle.py that
/// builds the bundles from them, with the bundles ignored by git. The
/// bundles are on disk too, as a worktree that ran the script has them.
pub fn design_system(dir: &Path, token: &str, bundle_css: &str, bundle_js: &str) {
    let system = dir.join("design/system");
    let components = system.join("components");
    std::fs::create_dir_all(components.join("src")).expect("creatable");
    let write = |path: PathBuf, text: &str| std::fs::write(path, text).expect("writable");
    write(
        system.join(".gitignore"),
        "tokens.css\ncomponents/bundle.css\ncomponents/bundle.js\n",
    );
    write(system.join("tokens.json"), &tokens_json(token));
    write(components.join("src/bundle.css"), bundle_css);
    write(components.join("src/bundle.js"), bundle_js);
    write(
        system.join("build-bundle.py"),
        "import pathlib\n\
         components = pathlib.Path(__file__).with_name('components')\n\
         for name in ('bundle.css', 'bundle.js'):\n    \
         (components / name).write_text((components / 'src' / name).read_text())\n",
    );
    write(components.join("bundle.css"), bundle_css);
    write(components.join("bundle.js"), bundle_js);
}
