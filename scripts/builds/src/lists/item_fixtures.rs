//! Scratch repos for the work queue's tests: a repo `demo` with an origin,
//! side worktrees with claims and branches, and a project `demo` over it.

use std::path::PathBuf;

use crate::checkout::git_text;
use crate::claim::write_claim;
use crate::checkout::{Checkout, git};
use crate::fail::{Fail, Result};
use crate::lists::fixtures::{NOON, stamp};
use crate::lists::store::Store;
use crate::lists::Lists;
use crate::testing::{Scratch, git as run_git, repo};
use crate::tool::Tool;
use std::collections::BTreeMap;

pub struct Demo {
    pub scratch: Scratch,
    pub tool: Tool,
    pub main: PathBuf,
    pub store: Store,
}

pub fn demo(name: &str) -> Demo {
    let scratch = Scratch::new(name);
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join("origin.git");
    run_git(&scratch.dir, &["init", "-q", "--bare", &origin.display().to_string()]);
    run_git(&main, &["remote", "add", "origin", &origin.display().to_string()]);
    run_git(&main, &["push", "-q", "origin", "main"]);
    let store = Store::named(&tool.places, "demo").unwrap();
    let mut state = Lists::empty("demo", vec!["demo".into()], BTreeMap::new());
    state.areas = vec!["Alpha".into(), "Beta".into()];
    store.create(state, &stamp(NOON), "lists init demo").unwrap();
    Demo { scratch, tool, main, store }
}

impl Demo {
    pub fn side(&self, n: usize) -> PathBuf {
        self.scratch.dir.join(format!("demo-worktrees/side-{n}"))
    }

    /// A claimed side worktree <n>, on a new branch <branch>.
    pub fn add_side(&self, n: usize, who: &str, branch: &str) -> PathBuf {
        let top = self.side(n);
        std::fs::create_dir_all(top.parent().unwrap()).unwrap();
        run_git(&self.main, &["worktree", "add", "-q", "-b", branch, &top.display().to_string(), "main"]);
        let checkout = Checkout::find(&top.display().to_string()).unwrap();
        write_claim(&checkout, who, NOON, None, None).unwrap();
        top
    }

    /// Commits a file in <top> and returns the commit.
    pub fn commit(&self, top: &std::path::Path, file: &str) -> String {
        std::fs::write(top.join(file), file).unwrap();
        run_git(top, &["add", file]);
        run_git(top, &["commit", "-q", "-m", file]);
        git_text(git(top).args(["rev-parse", "HEAD"]), "read HEAD").unwrap().trim().to_string()
    }

    /// Another branch in the repo, for a worktree that takes one branch.
    pub fn add_side_branch(&self, _top: &std::path::Path, branch: &str) {
        run_git(&self.main, &["branch", branch, "main"]);
    }

    pub fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.store.state_path()).unwrap()
    }

    pub fn brief(&self, name: &str) -> String {
        let path = self.scratch.dir.join(name);
        std::fs::write(&path, "brief").unwrap();
        path.display().to_string()
    }
}

pub fn words(args: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    all.extend(["--project".into(), "demo".into(), "--by".into(), "tester".into()]);
    all
}

pub fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}
