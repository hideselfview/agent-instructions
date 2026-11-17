//! Where one of a repo's app checkouts is: a side worktree on this Mac, or a
//! checkout on a host (app_remote.rs), whose git runs there over ssh. A build
//! runs its steps in it (app_build.rs) and a reset puts it on origin's main
//! (app_reset.rs).

use std::path::PathBuf;

use crate::app_config::app_checkout_top;
use crate::app_remote::RemoteApp;
use crate::checkout::{git, git_text};
use crate::fail::Result;
use crate::host::Ssh;
use crate::lane::Lane;
use crate::places::Places;

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
                git_text(command.args(args), &format!("git {}", args.join(" ")))
            }
            Site::Remote(remote) => remote.git(index, args),
        }
    }
}
