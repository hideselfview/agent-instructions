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
//! A job claims its worktree with the folder `<worktree>.job` (job_claim.rs).
//! prune keeps a claimed worktree while the `builds` process on this Mac that
//! made the claim runs, and removes the worktree of a claim whose process is
//! gone. prune takes the claim itself while it removes the worktree, so no
//! job starts there.

use crate::checkout::{file_name, worktrees_of};
use crate::fail::Result;
use crate::host::{self, Host, Ssh, clone_path, sh_quote};
use crate::job_claim::{Holder, Holders, RELEASE_GONE};
use crate::places::Places;
use crate::prune::Run;
use crate::remote::{JOB_WORKTREE_PREFIX, Transport, root_script};
use crate::text::note;

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
    let Some(rest) = name.strip_prefix(JOB_WORKTREE_PREFIX) else {
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

/// What prune does with a job worktree.
#[derive(Debug, PartialEq, Eq)]
enum Judged {
    /// A checkout here uses it, or a job claims it.
    Used,
    /// No checkout here uses it, for this reason, and prune removes it.
    Unused(String),
    /// No checkout here uses it, but its claim names a job whose pid builds
    /// cannot read. prune keeps it and says so.
    UnreadableClaim(String),
}

/// What prune does with <worktree>: it removes the worktree when it belongs
/// to no checkout among <checkouts> and no running job claims it, by
/// <holder>, which says whether the process that made a claim runs.
fn judge(
    worktree: &HostWorktree,
    checkouts: &[String],
    holder: impl Fn(&str) -> Option<Holder>,
) -> Judged {
    if belongs(&worktree.name, checkouts) {
        return Judged::Used;
    }
    let checkout = worktree
        .name
        .strip_prefix(JOB_WORKTREE_PREFIX)
        .unwrap_or(&worktree.name);
    let why = format!("no checkout of {} here is named {checkout}", worktree.repo);
    match &worktree.claim {
        Claim::Free => Judged::Unused(why),
        Claim::Starting => Judged::Used,
        Claim::Job(id) => match holder(id) {
            Some(Holder::Runs) => Judged::Used,
            Some(Holder::Gone) => Judged::Unused(format!(
                "{why}, and the process of job {id}, which claimed it, is gone"
            )),
            None => Judged::UnreadableClaim(format!(
                "{why}, but the job id {id:?} in its claim names no pid builds can check"
            )),
        },
    }
}

/// Removes <worktree> on the host and its build root there: takes its claim
/// as a job does, after removing the claim of the gone job <worktree> names,
/// so the removal fails when a job claimed it since the listing.
fn remove_script(host: &Host, worktree: &HostWorktree) -> String {
    let stale = match &worktree.claim {
        Claim::Job(id) => format!("release_gone \"$wt.job\" {}\n", sh_quote(id)),
        Claim::Free | Claim::Starting => String::new(),
    };
    format!(
        r#"{root}{RELEASE_GONE}wt="$HOME/dev/"{repo}-worktrees/{name}
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
    Ok(places.app_checkout_entries(repo)?.contains(&entry))
}

/// Removes the job worktrees on every recorded host that no checkout's jobs
/// use (or with <run> dry, says which it would remove). When a host does not
/// answer or a removal fails there, prune notes it and moves on to the next
/// one.
pub fn prune(places: &Places, ssh: &Ssh, run: Run) -> Result<()> {
    let holders = Holders::read()?;
    for host in host::read_remote(places)? {
        prune_host(places, ssh, &host, &holders, run)?;
    }
    Ok(())
}

fn prune_host(places: &Places, ssh: &Ssh, host: &Host, holders: &Holders, run: Run) -> Result<()> {
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
        let why = match judge(&worktree, here, |id| holders.holder(id)) {
            Judged::Used => continue,
            Judged::Unused(why) => why,
            Judged::UnreadableClaim(said) => {
                note(format!("kept {shown}: {said}"));
                continue;
            }
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
