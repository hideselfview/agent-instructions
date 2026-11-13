//! Scratch repos for the work queue's tests: a repo `demo` with an origin,
//! side worktrees with claims and branches, and a project `demo` over it.

use std::path::PathBuf;

use crate::checkout::git_text;
use crate::claim::write_claim;
use crate::checkout::{Checkout, git};
use crate::fail::{Fail, Result};
use crate::lists::fixtures::NOON;
use crate::lists::store::Store;
use crate::testing::{Scratch, git as run_git, repo};
use crate::tool::Tool;

pub struct Demo {
    pub scratch: Scratch,
    pub tool: Tool,
    pub main: PathBuf,
    pub store: Store,
}

pub fn demo(name: &str) -> Demo {
    demo_over(name, &["demo"])
}

/// A repo with an origin in the scratch folder: <repo>, and <repo>.git beside it.
fn add_repo_with_origin(scratch: &Scratch, repo_name: &str) -> PathBuf {
    let main = scratch.dir.join(repo_name);
    repo(&main, "target 1G\n");
    let origin = scratch.dir.join(format!("{repo_name}-origin.git"));
    run_git(&scratch.dir, &["init", "-q", "--bare", &origin.display().to_string()]);
    run_git(&main, &["remote", "add", "origin", &origin.display().to_string()]);
    run_git(&main, &["push", "-q", "origin", "main"]);
    main
}

/// A project `demo` over each repo of <repos> (the first is `main`).
pub fn demo_over(name: &str, repos: &[&str]) -> Demo {
    let scratch = Scratch::new(name);
    let tool = scratch.tool(0);
    let main = add_repo_with_origin(&scratch, repos[0]);
    for other in &repos[1..] {
        add_repo_with_origin(&scratch, other);
    }
    let store = crate::lists::fixtures::make_store(&tool.places, repos);
    Demo { scratch, tool, main, store }
}

impl Demo {
    /// A claimed side worktree <n>, on a new branch <branch>.
    pub fn add_side(&self, n: usize, who: &str, branch: &str) -> PathBuf {
        self.add_side_of("demo", n, who, branch)
    }

    /// `add_side` for the repo <repo> of the project (`demo_over`).
    pub fn add_side_of(&self, repo: &str, n: usize, who: &str, branch: &str) -> PathBuf {
        let main = self.scratch.dir.join(repo);
        let top = self.scratch.dir.join(format!("{repo}-worktrees/side-{n}"));
        std::fs::create_dir_all(top.parent().unwrap()).unwrap();
        run_git(&main, &["worktree", "add", "-q", "-b", branch, &top.display().to_string(), "main"]);
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

    /// Everything the store holds, for comparing before and after a command:
    /// the lists as read and the write log.
    pub fn state(&self) -> String {
        crate::lists::fixtures::state_of(&self.store)
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
