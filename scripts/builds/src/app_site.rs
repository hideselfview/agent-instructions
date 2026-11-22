//! Where one of a repo's app checkouts is: a side worktree on this Mac, or a
//! checkout on a host (app_remote.rs), whose git runs there over ssh. A build
//! runs its steps in it (app_build.rs) and a reset puts it on origin's main
//! (app_reset.rs).

use std::path::PathBuf;

use crate::app_config::{app_checkout_top, resolve, run_git, switch_to};
use crate::app_remote::{RemoteApp, fetched_local_source};
use crate::fail::{Fail, Result};
use crate::host::{Accept, Ssh, output_of};
use crate::lane::Lane;
use crate::places::Places;
use crate::side_git::{UNCOMMITTED_STATUS, paths_in_status};
use boards::{git, git_text};

/// One of a repo's app checkouts, on this Mac or on a host.
pub enum Site {
    /// The checkout's folder here, and the file Xcode reads its settings from
    /// for builds in it (xcode_cache.rs).
    Local(PathBuf, PathBuf),
    Remote(Box<RemoteApp>),
}

impl Site {
    /// The checkout <entry> of <repo>'s <lane>.
    pub fn of(places: &Places, ssh: &Ssh, repo: &str, lane: Lane, entry: &str) -> Result<Site> {
        Ok(match RemoteApp::find(places, ssh, repo, lane, entry)? {
            Some(remote) => Site::Remote(Box::new(remote)),
            None => {
                let top = app_checkout_top(places, repo, entry)?;
                let settings = crate::xcode_cache::settings_for(places, &top)?;
                Site::Local(top, settings)
            }
        })
    }

    /// The folder on this Mac that holds the checkout's commits, after a
    /// fetch from origin: the checkout itself here, or `local_source` for a
    /// checkout on a host, whose clone gets commits only from this Mac.
    pub fn fetched_source(&self, places: &Places, repo: &str) -> Result<PathBuf> {
        match self {
            Site::Local(top, _) => {
                run_git(top, &["fetch", "-q", "origin"])?;
                Ok(top.clone())
            }
            Site::Remote(_) => fetched_local_source(places, repo),
        }
    }

    /// What <script> printed, run with `sh` on the checkout's host (in no
    /// checkout); a failure says <what> and what the script wrote to stderr.
    fn print_of(&self, script: &str, what: &str) -> Result<String> {
        match self {
            Site::Local(..) => {
                let mut command = std::process::Command::new("sh");
                command.args(["-c", script]);
                output_of(command, what, Accept::Zero).map_err(Fail::Message)
            }
            Site::Remote(remote) => remote.on_host(script, what),
        }
    }

    /// The environment <lane>'s shots are drawn in on the checkout's host
    /// (shots_environment.rs), read from the host now.
    pub fn environment(&self, lane: Lane) -> Result<String> {
        crate::shots_environment::read(lane, &mut |script, what| self.print_of(script, what))
    }

    /// The commit the checkout is on.
    pub fn head(&mut self) -> Result<String> {
        match self {
            Site::Local(top, _) => resolve(top, "HEAD"),
            Site::Remote(remote) => remote.head(),
        }
    }

    /// The checkout's folder, as a message shows it.
    pub fn shown(&mut self) -> Result<String> {
        Ok(match self {
            Site::Local(top, _) => top.display().to_string(),
            Site::Remote(remote) => format!("{} on host {}", remote.locate()?, remote.host.name),
        })
    }

    /// What `git <args>` printed in the checkout, run with the index file
    /// <index> when given.
    pub fn git(&mut self, index: Option<&str>, args: &[&str]) -> Result<String> {
        match self {
            Site::Local(top, _) => {
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
            Site::Local(top, _) => switch_to(top, commit, true),
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
