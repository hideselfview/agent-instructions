//! Where one of a repo's app checkouts is: a side worktree on this Mac, or a
//! checkout on a host (app_remote.rs), whose git runs there over ssh. A build
//! runs its steps in it (app_build.rs) and a reset puts it on origin's main
//! (app_reset.rs).

use std::path::PathBuf;

use crate::app_config::{app_checkout_top, resolve, run_git, switch_to};
use crate::app_remote::{RemoteApp, fetched_local_source};
use crate::fail::Result;
use crate::host::Ssh;
use crate::lane::Lane;
use crate::places::Places;
use crate::side_git::{UNCOMMITTED_STATUS, paths_in_status};
use boards::{git, git_text};

/// One of a repo's app checkouts, on this Mac or on a host.
pub enum Site {
    Local(PathBuf),
    Remote(Box<RemoteApp>),
}

impl Site {
    /// The checkout <entry> of <repo>'s <lane>.
    pub fn of(places: &Places, ssh: &Ssh, repo: &str, lane: Lane, entry: &str) -> Result<Site> {
        Ok(match RemoteApp::find(places, ssh, repo, lane, entry)? {
            Some(remote) => Site::Remote(Box::new(remote)),
            None => Site::Local(app_checkout_top(places, repo, entry)?),
        })
    }

    /// The folder on this Mac that holds the checkout's commits, after a
    /// fetch from origin: the checkout itself here, or `local_source` for a
    /// checkout on a host, whose clone gets commits only from this Mac.
    pub fn fetched_source(&self, places: &Places, repo: &str) -> Result<PathBuf> {
        match self {
            Site::Local(top) => {
                run_git(top, &["fetch", "-q", "origin"])?;
                Ok(top.clone())
            }
            Site::Remote(_) => fetched_local_source(places, repo),
        }
    }

    /// The commit the checkout is on.
    pub fn head(&mut self) -> Result<String> {
        match self {
            Site::Local(top) => resolve(top, "HEAD"),
            Site::Remote(remote) => remote.head(),
        }
    }

    /// The checkout's folder, as a message shows it.
    pub fn shown(&mut self) -> Result<String> {
        Ok(match self {
            Site::Local(top) => top.display().to_string(),
            Site::Remote(remote) => format!("{} on host {}", remote.locate()?, remote.host.name),
        })
    }

    /// What `git <args>` printed in the checkout, run with the index file
    /// <index> when given.
    pub fn git(&mut self, index: Option<&str>, args: &[&str]) -> Result<String> {
        match self {
            Site::Local(top) => {
                let mut command = git(top);
                if let Some(index) = index {
                    command.env("GIT_INDEX_FILE", index);
                }
                Ok(git_text(
                    command.args(args),
                    &format!("git {}", args.join(" ")),
                )?)
            }
            Site::Remote(remote) => remote.git(index, args),
        }
    }

    /// The paths `git status` names in the checkout: changed, staged and
    /// untracked files alike.
    pub fn changes(&mut self) -> Result<Vec<String>> {
        Ok(paths_in_status(&self.git(None, &UNCOMMITTED_STATUS)?)
            .into_iter()
            .collect())
    }

    /// Detaches the checkout at <commit> over its local changes. On this Mac
    /// the switch holds out against every signal but SIGKILL (`switch_to`).
    pub fn force_to(&mut self, commit: &str) -> Result<()> {
        match self {
            Site::Local(top) => switch_to(top, commit, true),
            Site::Remote(remote) => remote
                .git(None, &["checkout", "-q", "--detach", "--force", commit])
                .map(|_| ()),
        }
    }

    /// Puts the checkout back on its commit: the commit's files over every
    /// change, and no file that git neither tracks nor ignores.
    pub fn put_back(&mut self) -> Result<()> {
        self.force_to("HEAD")?;
        self.git(None, &["clean", "-q", "-f", "-d"]).map(|_| ())
    }
}
