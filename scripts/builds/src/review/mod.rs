//! `builds review`: reviews a change against the rules of agent-instructions,
//! one reviewer per group of rules (`review/groups.toml`), and records every
//! review in the project's store.
//!
//! ```text
//! builds review [run] WORKTREE [BASE] [--landing KEY] [--item N]... [--rules-root DIR] [--reviewer PROGRAM]
//! builds review settle FINDING confirmed|dropped --why TEXT
//! builds review show LANDING
//! builds review groups [--rules-root DIR] [--project P] [--changed PATH]...
//! ```
//!
//! `run` reviews the worktree's commits past BASE (default `origin/main`). The
//! groups whose rules cover a changed path each get one `claude -p`. A review
//! of a KEY that an earlier review left unsettled reruns only the groups that
//! found something (or failed) and passes them what the coordinator settled.
//! It exits 1 when a blocking finding stands or a reviewer failed.

mod call;
mod check;
mod groups;
mod prompt;
mod record;
mod render;
mod rules;
mod run;

use std::path::{Path, PathBuf};

use crate::checkout::{Checkout, git};
use crate::db;
use crate::fail;
use crate::fail::{Fail, Result};
use crate::lists::Stamp;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::{Store, project_of, resolve_by};
use crate::tool::Tool;

use prompt::Settled;
use record::Kind;

const USAGE: &str = "usage: builds review [run] WORKTREE [BASE] [--landing KEY] [--item N]... [--rules-root DIR] [--reviewer PROGRAM]\n       builds review settle FINDING confirmed|dropped --why TEXT\n       builds review show LANDING\n       builds review groups [--rules-root DIR] [--project P] [--changed PATH]...";

/// The checkout the rules and groups are read from: `--rules-root`, else the
/// one `install.sh` links at ~/.codex/agent-instructions.
fn rules_root(args: &Args) -> Result<PathBuf> {
    if let Some(root) = args.value("rules-root")? {
        return Ok(PathBuf::from(root));
    }
    match std::env::var_os("HOME") {
        Some(home) => Ok(PathBuf::from(home).join(".codex/agent-instructions")),
        None => fail!("HOME is not set; name the checkout with --rules-root"),
    }
}

pub fn cmd_review(tool: &Tool, args: &[String]) -> Result<()> {
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "groups" => list_groups(rest),
        "settle" => settle(tool, rest),
        "show" => show(tool, rest),
        "run" => run_command(tool, rest),
        _ => run_command(tool, args),
    }
}

/// What one review needs.
pub struct Inputs<'a> {
    pub store: &'a Store,
    pub project: &'a str,
    pub repo: &'a str,
    pub worktree: &'a Path,
    pub base: &'a str,
    /// Names the landing across reruns.
    pub key: &'a str,
    pub root: &'a Path,
    pub reviewer: &'a Path,
    pub stamp: &'a Stamp,
    /// (path, text) of the landing's brief.
    pub brief: Option<(String, String)>,
}

/// What a review found.
pub struct Reported {
    pub landing: Option<i64>,
    /// Everything to print.
    pub text: String,
    /// The words of a refusal when a blocking finding stands or a reviewer failed.
    pub refusal: Option<String>,
}

/// Reviews the worktree's change past the base and records it.
pub fn review(inputs: &Inputs) -> Result<Reported> {
    let diff = run::diff_of(inputs.worktree, inputs.base)?;
    if diff.text.trim().is_empty() {
        return Ok(Reported {
            landing: None,
            text: format!("nothing to review: no changes past {}\n", inputs.base),
            refusal: None,
        });
    }
    let material = run::Material::load(inputs.root, inputs.project)?;
    // What an earlier review of this landing left standing.
    let (prior, rerun, settled) = match db::open_reader(&inputs.store.database())? {
        None => (None, Vec::new(), Vec::new()),
        Some(conn) => match record::open_landing(&conn, inputs.key)? {
            None => (None, Vec::new(), Vec::new()),
            Some(landing) => {
                let groups = record::groups_to_rerun(&conn, landing)?;
                let settled: Vec<(String, Settled)> = record::findings_of_landing(&conn, landing)?
                    .into_iter()
                    .filter_map(|f| {
                        f.settled.map(|(how, why)| {
                            (
                                f.group,
                                Settled {
                                    rule: f.rule,
                                    file: f.file,
                                    code: f.code,
                                    how,
                                    why,
                                },
                            )
                        })
                    })
                    .collect();
                (Some(landing), groups, settled)
            }
        },
    };
    let rerun_only = prior.is_some() && !rerun.is_empty();
    let mut picked = material.groups.pick(&material.rules, &diff.changed);
    if rerun_only {
        picked.retain(|(group, _)| rerun.contains(&group.name));
    }
    if picked.is_empty() {
        return Ok(Reported {
            landing: prior,
            text: "no group's rules cover the changed paths: nothing to review\n".to_string(),
            refusal: None,
        });
    }
    let settled_for = |group: &str| -> Vec<Settled> {
        settled
            .iter()
            .filter(|(g, _)| g == group)
            .map(|(_, s)| s.clone())
            .collect()
    };
    let calls = run::ask_groups(
        &material,
        inputs.reviewer,
        &picked,
        &diff,
        inputs.brief.as_ref(),
        &settled_for,
    );
    let stamp = inputs.stamp;
    let (landing, run_id, stored_calls, findings) =
        inputs
            .store
            .write(stamp, &format!("review {}", inputs.key), |db| {
                let landing = match prior {
                    Some(id) => id,
                    None => record::add_landing(db, inputs.key, inputs.repo, stamp.at)?,
                };
                let (run_id, _) = record::add_run(
                    db,
                    &record::NewRun {
                        landing: Some(landing),
                        kind: Kind::Review,
                        label: None,
                        at: stamp.at,
                        base: &diff.base,
                        head: &diff.head,
                    },
                    &calls,
                )?;
                // A finding the coordinator dropped on an earlier run, raised
                // again, is the same finding: it stays dropped.
                let earlier: Vec<record::Finding> = record::findings_of_landing(db, landing)?
                    .into_iter()
                    .filter(|f| f.run_id != run_id)
                    .collect();
                for finding in record::findings_of_run(db, run_id)? {
                    if !finding.counts() {
                        continue;
                    }
                    let same = earlier.iter().find(|e| {
                        matches!(&e.settled, Some((how, _)) if how == "dropped")
                            && e.rule == finding.rule
                            && e.file == finding.file
                            && check::squash(&e.code) == check::squash(&finding.code)
                    });
                    if let Some(same) = same {
                        record::inherit_drop(db, finding.id, same, stamp.at)?;
                    }
                }
                let stored = record::calls_of_run(db, run_id)?;
                let findings = record::findings_of_run(db, run_id)?;
                let stands = findings.iter().any(|f| f.refuses())
                    || stored.iter().any(|c| c.failure.is_some());
                if !stands {
                    record::close_landing(db, landing, stamp.at)?;
                }
                Ok((landing, run_id, stored, findings))
            })?;
    let mut text = format!(
        "review of {} (landing {landing}, run {run_id}) over {}...{}\n",
        inputs.key,
        short(&diff.base),
        short(&diff.head)
    );
    if rerun_only {
        text.push_str("rerun: only the groups that found something or failed before\n");
    }
    text.push_str(&render::run_report(&stored_calls, &findings));
    let standing: Vec<&record::Finding> = findings.iter().filter(|f| f.refuses()).collect();
    let failed: Vec<&record::Call> = stored_calls
        .iter()
        .filter(|c| c.failure.is_some())
        .collect();
    let refusal =
        (!standing.is_empty() || !failed.is_empty()).then(|| render::refusal(&standing, &failed));
    Ok(Reported {
        landing: Some(landing),
        text,
        refusal,
    })
}

fn short(rev: &str) -> &str {
    &rev[..rev.len().min(10)]
}

fn run_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["landing", "item", "rules-root", "reviewer"], &[])?;
    let (worktree, base) = match args.positional() {
        [worktree] => (worktree.as_str(), "origin/main"),
        [worktree, base] => (worktree.as_str(), base.as_str()),
        _ => fail!("{USAGE}"),
    };
    let checkout = Checkout::find(worktree)?;
    let worktree = checkout.top.clone();
    let project = match args.value("project")? {
        Some(project) => project.to_string(),
        None => match project_of(&tool.places, &checkout)? {
            Some(project) => project,
            None => fail!(
                "repo {} is in no project of the lists, so there is no store to record a review in; say which with --project",
                checkout.repo()
            ),
        },
    };
    let store = Store::named(&tool.places, &project)?;
    let key = match args.value("landing")? {
        Some(key) => key.to_string(),
        None => default_key(&worktree)?,
    };
    let stamp = Stamp {
        at: tool.now,
        by: resolve_by(&args, &worktree).or_else(|_| resolve_by(&args, &current_dir()?))?,
    };
    let brief = brief_of(&store, &args, &worktree)?;
    let root = rules_root(&args)?;
    let reviewer = PathBuf::from(args.value("reviewer")?.unwrap_or("claude"));
    let reported = review(&Inputs {
        store: &store,
        project: &project,
        repo: &checkout.repo(),
        worktree: &worktree,
        base,
        key: &key,
        root: &root,
        reviewer: &reviewer,
        stamp: &stamp,
        brief,
    })?;
    print!("{}", reported.text);
    match reported.refusal {
        Some(refusal) => {
            print!("{refusal}");
            if let Some(landing) = reported.landing {
                println!("builds review show {landing} lists every run of this landing");
            }
            Err(Fail::Exit(1))
        }
        None => Ok(()),
    }
}

/// The branch the worktree is on, else its HEAD.
fn default_key(worktree: &Path) -> Result<String> {
    let text = |args: &[&str]| -> Option<String> {
        let output = git(worktree).args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    if let Some(branch) = text(&["symbolic-ref", "-q", "--short", "HEAD"]) {
        return Ok(format!("branch:{branch}"));
    }
    match text(&["rev-parse", "--short", "HEAD"]) {
        Some(head) => Ok(format!("detached:{head}")),
        None => fail!("{} has no HEAD to review", worktree.display()),
    }
}

/// The brief of the first `--item` that has one, read from the worktree.
fn brief_of(store: &Store, args: &Args, worktree: &Path) -> Result<Option<(String, String)>> {
    let ids = args.values("item");
    if ids.is_empty() {
        return Ok(None);
    }
    let lists = store.load()?;
    for id in ids {
        let id = crate::lists::Id::parse(id)?;
        let Some(item) = lists.items.iter().find(|item| item.id == id) else {
            fail!("--item {}: there is no open item {}", id.0, id.0);
        };
        if let Some(brief) = &item.brief {
            let path = brief.split('#').next().unwrap_or(brief);
            let text = std::fs::read_to_string(worktree.join(path))
                .unwrap_or_else(|_| format!("(the brief file {path} is not in the worktree)"));
            return Ok(Some((brief.clone(), text)));
        }
    }
    Ok(None)
}

/// `builds review settle FINDING confirmed|dropped --why TEXT`.
fn settle(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["why"], &[])?;
    let [finding, how] = args.exactly(2, USAGE)? else {
        unreachable!("exactly two")
    };
    let id: i64 = finding
        .parse()
        .map_err(|_| Fail::Message(format!("'{finding}' is not a finding number")))?;
    let why = args.why()?;
    let cwd = current_dir()?;
    let store = Store::resolve(&tool.places, &args, &cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, &cwd)?;
    store.write(&stamp, &format!("review settle {id}"), |db| {
        record::settle(db, id, how, why, stamp.at, &stamp.by)
    })?;
    println!("finding {id} settled {how}");
    Ok(())
}

/// `builds review show LANDING`.
fn show(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [landing] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let id: i64 = landing
        .parse()
        .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))?;
    let cwd = current_dir()?;
    let store = Store::resolve(&tool.places, &args, &cwd)?;
    let Some(conn) = db::open_reader(&store.database())? else {
        fail!("there is no store for project {}", store.project);
    };
    let (key, repo, closed) = record::landing(&conn, id)?;
    println!(
        "landing {id}: {key} ({repo}), review {}",
        if closed { "passed" } else { "not passed" }
    );
    for record::RunRow {
        id: run,
        kind,
        base,
        head,
    } in record::runs_of_landing(&conn, id)?
    {
        println!("{kind} {run} over {}...{}", short(&base), short(&head));
        print!(
            "{}",
            render::run_report(
                &record::calls_of_run(&conn, run)?,
                &record::findings_of_run(&conn, run)?
            )
        );
    }
    Ok(())
}

/// Prints each group, and fails when the grouping is not whole. With
/// `--changed`, prints only the groups those paths run.
fn list_groups(args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root", "changed"], &[])?;
    args.exactly(0, USAGE)?;
    let root = rules_root(&args)?;
    let groups = groups::Groups::load(&root)?;
    let every = rules::load(&root, None)?;
    groups::require_whole(&groups, &every)?;
    let changed: Vec<String> = args
        .values("changed")
        .into_iter()
        .map(str::to_string)
        .collect();
    if changed.is_empty() {
        for group in &groups.groups {
            print_group(group, group.rules.iter().map(String::as_str));
        }
        return Ok(());
    }
    let project_rules = rules::load(&root, args.value("project")?)?;
    for (group, members) in groups.pick(&project_rules, &changed) {
        print_group(group, members.iter().map(|rule| rule.slug.as_str()));
    }
    Ok(())
}

fn print_group<'a>(group: &groups::Group, slugs: impl Iterator<Item = &'a str>) {
    println!("{} ({}): {}", group.name, group.model, group.question);
    for slug in slugs {
        println!("  {slug}");
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
