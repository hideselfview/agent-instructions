//! Removing the worktrees on the hosts that no checkout's jobs use. A job runs
//! in the worktree on its host that belongs to the checkout it comes from
//! (`remote::pinned_name`): `~/dev/<repo>-worktrees/remote-<the checkout's
//! folder name>`, or `remote-<name>.<n>` while another job holds that one.
//! Once the checkout is gone from this Mac (a retired side worktree), or the
//! worktree has a name from before builds named them after checkouts (the
//! numbered remote-1, remote-2 and on), no job uses it again, and it keeps its
//! build folders on the host. `builds prune` asks each host for its job
//! worktrees and removes each one that belongs to no checkout of this Mac's
//! clone of its repo, with its build root on the host, as it deletes a build
//! root here whose checkout is gone.
//!
//! A job claims its worktree with the folder `<worktree>.job`, which holds the
//! job's id, `<pid>-<nanoseconds>`, whose pid is the `builds` process on this
//! Mac that runs the job and releases the claim when it ends. prune keeps a
//! claimed worktree while that process runs, and removes the worktree of a
//! claim whose process is gone, since nothing releases that claim. It takes
//! the claim itself while it removes the worktree, so no job starts there.

use crate::checkout::{file_name, worktrees_of};
use crate::fail::Result;
use crate::host::{self, Host, Ssh, clone_path, sh_quote};
use crate::lane::Lane;
use crate::places::Places;
use crate::prune::Run;
use crate::remote::{Transport, root_script};
use crate::text::note;

/// The prefix of the name of every worktree a job runs in.
const PREFIX: &str = "remote-";

/// Who holds a job worktree's claim.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Claim {
    /// No job claims it.
    Free,
    /// The job with this id claims it.
    Job(String),
    /// A claim folder holds no id: a job is claiming it now.
    Starting,
}

/// A job worktree on a host, as `list_script` printed it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HostWorktree {
    repo: String,
    name: String,
    claim: Claim,
}

/// Runs on a host to print one `worktree <repo> <name> <claim>` line for
/// each job worktree there, where <claim> is `-` for none, `?` for a claim
/// folder with no id yet, and the job's id otherwise.
const LIST_SCRIPT: &str = r#"for wts in "$HOME"/dev/*-worktrees; do
  [ -d "$wts" ] || continue
  repo=$(basename "$wts" -worktrees)
  for wt in "$wts"/remote-*; do
    [ -e "$wt/.git" ] || continue
    claim=-
    if [ -d "$wt.job" ]; then
      claim=$(cat "$wt.job/id" 2>/dev/null | tr -d ' \n')
      [ -n "$claim" ] || claim='?'
    fi
    echo "worktree $repo $(basename "$wt") $claim"
  done
done"#;

/// Reads what `LIST_SCRIPT` printed.
fn parse_list(text: &str) -> Vec<HostWorktree> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.strip_prefix("worktree ")?.split(' ');
            let (repo, name, claim) = (words.next()?, words.next()?, words.next()?);
            Some(HostWorktree {
                repo: repo.to_string(),
                name: name.to_string(),
                claim: match claim {
                    "-" => Claim::Free,
                    "?" => Claim::Starting,
                    id => Claim::Job(id.to_string()),
                },
            })
        })
        .collect()
}

/// Does the job worktree <name> belong to one of <checkouts> (the folder
/// names of the checkouts of its repo on this Mac): is it
/// `remote-<checkout>` or `remote-<checkout>.<n>`?
fn belongs(name: &str, checkouts: &[String]) -> bool {
    let Some(rest) = name.strip_prefix(PREFIX) else {
        return false;
    };
    checkouts.iter().any(|checkout| {
        rest == checkout
            || rest
                .strip_prefix(checkout.as_str())
                .and_then(|after| after.strip_prefix('.'))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Is the `builds` process on this Mac that runs job <id> still running?
/// An id whose pid builds cannot read counts as running.
fn job_runs(id: &str) -> bool {
    let Some(pid) = id
        .split_once('-')
        .and_then(|(pid, _)| pid.parse::<libc::pid_t>().ok())
    else {
        return true;
    };
    // SAFETY: signal 0 only checks that the process exists.
    let exists = unsafe { libc::kill(pid, 0) == 0 };
    exists || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Why <worktree> goes, or None when it stays: it belongs to no checkout
/// among <checkouts> and no running job claims it.
fn unused(
    worktree: &HostWorktree,
    checkouts: &[String],
    job_runs: impl Fn(&str) -> bool,
) -> Option<String> {
    if belongs(&worktree.name, checkouts) {
        return None;
    }
    let checkout = worktree.name.strip_prefix(PREFIX).unwrap_or(&worktree.name);
    let why = format!("no checkout of {} here is named {checkout}", worktree.repo);
    match &worktree.claim {
        Claim::Free => Some(why),
        Claim::Starting => None,
        Claim::Job(id) if job_runs(id) => None,
        Claim::Job(id) => Some(format!(
            "{why}, and the process of job {id}, which claimed it, is gone"
        )),
    }
}

/// Removes <worktree> on the host and its build root there: takes its claim
/// as a job does, after removing the claim of the gone job <worktree> names,
/// so the removal fails when a job claimed it since the listing.
fn remove_script(host: &Host, worktree: &HostWorktree) -> String {
    let stale = match &worktree.claim {
        Claim::Job(id) => format!(
            "[ \"$(cat \"$wt.job/id\" 2>/dev/null | tr -d ' \\n')\" = {id} ] && rm -rf \"$wt.job\"\n",
            id = sh_quote(id)
        ),
        Claim::Free | Claim::Starting => String::new(),
    };
    format!(
        r#"{root}wt="$HOME/dev/"{repo}-worktrees/{name}
{stale}if ! mkdir "$wt.job" 2>/dev/null; then echo "a job claimed $wt" >&2; exit 94; fi
git -C "$HOME/"{clone} worktree remove --force "$wt" && rm -rf "$BUILDS_ROOT"/{repo}/{name}
status=$?
rm -rf "$wt.job"
exit $status"#,
        root = root_script(host),
        repo = sh_quote(&worktree.repo),
        name = sh_quote(&worktree.name),
        clone = sh_quote(&clone_path(&worktree.repo)),
    )
}

/// The folder names of the checkouts of <repo> on this Mac, the worktrees of
/// its clone `~/dev/<repo>`, or None when this Mac has no such clone.
fn checkouts_here(places: &Places, repo: &str) -> Result<Option<Vec<String>>> {
    let clone = places.home.join(clone_path(repo));
    if !clone.join(".git").exists() {
        return Ok(None);
    }
    Ok(Some(
        worktrees_of(&clone)?
            .iter()
            .map(|path| file_name(path))
            .collect(),
    ))
}

/// Is <name> one of <repo>'s app checkouts on <host>, which builds never
/// removes here?
fn app_checkout(places: &Places, repo: &str, host: &str, name: &str) -> Result<bool> {
    let entry = crate::app_remote::entry_of(host, name);
    for lane in Lane::ALL {
        if places.app_checkouts_of(repo, lane)?.contains(&entry) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Removes the job worktrees on every recorded host that no checkout's jobs
/// use (or with <run> dry, says which it would remove). A host that does not
/// answer, or a removal that fails there, is noted and passed over.
pub fn prune(places: &Places, ssh: &Ssh, run: Run) -> Result<()> {
    for host in host::read_remote(places)? {
        prune_host(places, ssh, &host, run)?;
    }
    Ok(())
}

fn prune_host(places: &Places, ssh: &Ssh, host: &Host, run: Run) -> Result<()> {
    let transport = Transport { places, ssh, host };
    let listed = match transport.ask(LIST_SCRIPT, "listing the job worktrees")? {
        Ok(listed) => parse_list(&listed),
        Err(said) => {
            note(format!("kept host {}'s job worktrees: {said}", host.name));
            return Ok(());
        }
    };
    let mut checkouts = std::collections::BTreeMap::new();
    for worktree in listed {
        let repo = &worktree.repo;
        if !checkouts.contains_key(repo) {
            checkouts.insert(repo.clone(), checkouts_here(places, repo)?);
        }
        let shown = format!(
            "host {}'s worktree ~/dev/{repo}-worktrees/{}",
            host.name, worktree.name
        );
        let Some(here) = &checkouts[repo] else {
            note(format!(
                "kept {shown}: this Mac has no clone of {repo} at ~/{} to tell which checkouts it has",
                clone_path(repo)
            ));
            continue;
        };
        let Some(why) = unused(&worktree, here, job_runs) else {
            continue;
        };
        if app_checkout(places, repo, &host.name, &worktree.name)? {
            continue;
        }
        if run.dry() {
            note(format!("would remove {shown} and its build root ({why})"));
            continue;
        }
        match transport.ask(&remove_script(host, &worktree), "removing a job worktree")? {
            Ok(_) => note(format!("removed {shown} and its build root ({why})")),
            Err(said) => note(format!("kept {shown}: {said}")),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "remote_worktrees_tests.rs"]
mod tests;
