//! `builds review`: reviews a change against the rules of agent-instructions,
//! one reviewer per group of rules (`rules/groups.toml`), and records every
//! review in the project's store.
//!
//! ```text
//! builds review [run] WORKTREE [BASE] [--landing KEY] [--item N]... [--head SHA] [--record-only] [--rules-root DIR] [--reviewer PROGRAM]
//! builds review settle FINDING confirmed|dropped --why TEXT
//! builds review show LANDING
//! builds review groups [--rules-root DIR] [--project P] [--changed PATH]...
//! builds review replay [--groups FILE] [--case NAME]... [--source REPO] [--model M] [--rules-root DIR] [--reviewer PROGRAM]
//! builds review audit LANDING --repo-dir DIR
//! builds review sample --landing KEY --repo-dir DIR
//! builds review misses | rates
//! builds review case add ...
//! ```
//!
//! `run` reviews the worktree's commits past BASE (default `origin/main`). The
//! groups whose rules cover a changed path each get one `claude -p`. A review
//! of a KEY that an earlier review left unsettled reruns only the groups that
//! found something (or failed) and passes them what the coordinator settled.
//! It exits 1 when a blocking finding stands or a reviewer failed.

mod audit;
mod call;
mod cases;
mod check;
#[cfg(test)]
mod fakes;
mod files;
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
use crate::fail::{Context, Fail, Result};
use crate::lists::Stamp;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::{Store, project_of, resolve_by};
use crate::tool::Tool;

use prompt::Settled;
use record::Kind;

const USAGE: &str = "usage: builds review [run] WORKTREE [BASE] [--landing KEY] [--item N]... [--head SHA] [--record-only] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review settle FINDING confirmed|dropped --why TEXT\n       builds review show LANDING\n       builds review groups [--rules-root DIR] [--project P] [--changed PATH]...\n       builds review replay [--groups FILE] [--case NAME]... [--source REPO] [--model opus|sonnet|haiku] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review audit LANDING --repo-dir DIR [--rules-root DIR] [--reviewer PROGRAM]\n       builds review sample --landing KEY --repo-dir DIR [--roll N] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review misses\n       builds review rates [--rules-root DIR]\n       builds review case add LANDING FINDING --repo-dir DIR --rules-root DIR [--name NAME]\n       builds review case add --rule SLUG --file PATH --commits BASE..HEAD --project P --name NAME --repo-dir DIR --rules-root DIR\n       builds review case add --clean-for SLUG --commits BASE..HEAD --project P --name NAME --repo-dir DIR --rules-root DIR";

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
        "replay" => replay(tool, rest),
        "audit" => audit_command(tool, rest),
        "sample" => sample(tool, rest),
        "misses" => misses(tool, rest),
        "rates" => rates(tool, rest),
        "case" => case_command(tool, rest),
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
    /// The commit to review, read from a copy of its files; the worktree's
    /// HEAD, read in place, when none.
    pub head: Option<&'a str>,
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
    let diff = match inputs.head {
        Some(head) => {
            let mut diff = run::diff_between(inputs.worktree, inputs.base, head)?;
            if let Some(source) = &mut diff.source {
                source.in_place = false;
            }
            diff
        }
        None => run::diff_of(inputs.worktree, inputs.base)?,
    };
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
    )
    .calls;
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
                        grouping: &material.grouping,
                        rules_version: &run::rules_version(inputs.root),
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
    let args = Args::parse(
        args,
        &["landing", "item", "rules-root", "reviewer", "head"],
        &["record-only"],
    )?;
    if args.switch("record-only") {
        return record_only(tool, &args);
    }
    let reported = run_review(tool, &args)?.0;
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

/// `builds review run ... --record-only`: the review of a landing that is on
/// main already. It reads the commits as they were when the landing pushed
/// (`--head`), keeps everything it asks and answers in the store, writes what
/// it found to a log in the store's folder and never fails.
fn record_only(tool: &Tool, args: &Args) -> Result<()> {
    let project = args.require("project")?;
    let log_dir = Store::named(&tool.places, project)?.dir.join("review-logs");
    let text = match run_review(tool, args) {
        Ok((reported, head)) => {
            let mut text = format!("record-only review of {}\n{}", short(&head), reported.text);
            if let Some(refusal) = reported.refusal {
                text.push_str(&refusal);
            }
            text
        }
        Err(why) => format!(
            "the review failed; the landing was not refused: {}\n",
            why.text()
        ),
    };
    let key = args.value("landing")?.unwrap_or("review");
    let head = args.value("head")?.unwrap_or("head");
    let name = format!(
        "{}-{}.log",
        key.replace(|c: char| !c.is_ascii_alphanumeric() && c != ',', "-"),
        short(head)
    );
    let write =
        std::fs::create_dir_all(&log_dir).and_then(|()| std::fs::write(log_dir.join(&name), &text));
    if let Err(error) = write {
        eprintln!("builds: writing the review log {name}: {error}\n{text}");
    }
    Ok(())
}

/// Runs the review `builds review run` names; returns what it found and the
/// commit it reviewed.
fn run_review(tool: &Tool, args: &Args) -> Result<(Reported, String)> {
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
        by: resolve_by(args, &worktree).or_else(|_| resolve_by(args, &current_dir()?))?,
    };
    let brief = brief_of(&store, args, &worktree)?;
    let root = rules_root(args)?;
    let reviewer = PathBuf::from(args.value("reviewer")?.unwrap_or("claude"));
    let head = args.value("head")?;
    let reported = review(&Inputs {
        store: &store,
        project: &project,
        repo: &checkout.repo(),
        worktree: &worktree,
        base,
        head,
        key: &key,
        root: &root,
        reviewer: &reviewer,
        stamp: &stamp,
        brief,
    })?;
    Ok((reported, head.unwrap_or("HEAD").to_string()))
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
        ..
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

/// `builds review replay`: runs a grouping over every case of the checkout's
/// `review/cases` and prints the recall, the false blocking findings and the
/// tokens. Each replayed case is recorded in its project's store.
fn replay(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "groups",
            "case",
            "rules-root",
            "reviewer",
            "source",
            "model",
        ],
        &[],
    )?;
    args.exactly(0, USAGE)?;
    let root = rules_root(&args)?;
    let reviewer = PathBuf::from(args.value("reviewer")?.unwrap_or("claude"));
    let source = args.value("source")?.map(PathBuf::from);
    let mut all = cases::load(&root)?;
    let wanted = args.values("case");
    if !wanted.is_empty() {
        for name in &wanted {
            if !all.iter().any(|c| c.name == *name) {
                fail!("there is no case {name}");
            }
        }
        all.retain(|c| wanted.contains(&c.name.as_str()));
    }
    if all.is_empty() {
        fail!("{} holds no cases", root.join(cases::DIR).display());
    }
    let alternative = match args.value("groups")? {
        Some(file) => Some(groups::Groups::parse(
            &std::fs::read_to_string(file).context(|| format!("reading {file}"))?,
        )?),
        None => None,
    };
    let cwd = current_dir()?;
    let stamp = Stamp::resolve(tool.now, &args, &cwd)?;
    let rules_version = run::rules_version(&root);
    let results = cases::replay_all(
        &root,
        &all,
        &reviewer,
        alternative.as_ref(),
        source.as_deref(),
        args.value("model")?,
    )?;
    for (case, result) in all.iter().zip(&results) {
        let store = Store::named(&tool.places, &case.project)?;
        store.write(&stamp, &format!("review replay {}", case.name), |db| {
            record::add_run(
                db,
                &cases::run_of(case, stamp.at, &rules_version),
                &result.calls,
            )
            .map(|_| ())
        })?;
    }
    print!("{}", cases::report(&results));
    if cases::totals(&results).failed_calls > 0 {
        return Err(Fail::Exit(1));
    }
    Ok(())
}

/// The project's store for a command that names no worktree.
fn store_of(tool: &Tool, args: &Args) -> Result<Store> {
    Store::resolve(&tool.places, args, &current_dir()?)
}

/// `rule` mapped to the name of its group in the checkout's groups file.
fn group_names(root: &Path) -> Result<impl Fn(&str) -> Option<String>> {
    let groups = groups::Groups::load(root)?;
    Ok(move |rule: &str| {
        groups
            .groups
            .iter()
            .find(|g| g.rules.iter().any(|r| r == rule))
            .map(|g| g.name.clone())
    })
}

/// `builds review audit LANDING --repo-dir DIR`: audits a landing that passed.
fn audit_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["repo-dir", "rules-root", "reviewer", "seed"], &[])?;
    let [landing] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let landing: i64 = landing
        .parse()
        .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))?;
    let store = store_of(tool, &args)?;
    let root = rules_root(&args)?;
    let reviewer = PathBuf::from(args.value("reviewer")?.unwrap_or("claude"));
    let repo_dir = PathBuf::from(args.require("repo-dir")?);
    let stamp = Stamp {
        at: tool.now,
        by: args.value("by")?.unwrap_or("builds audit").to_string(),
    };
    let seed = match args.value("seed")? {
        Some(seed) => seed
            .parse()
            .map_err(|_| Fail::Message(format!("'{seed}' is not a number")))?,
        None => clock_bits(),
    };
    let text = audit::audit(&audit::Inputs {
        store: &store,
        project: &store.project,
        repo_dir: &repo_dir,
        landing,
        root: &root,
        reviewer: &reviewer,
        stamp: &stamp,
        seed,
    })?;
    print!("{text}");
    Ok(())
}

/// Bits that differ from one run to the next.
fn clock_bits() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1);
    (now as u64) ^ (u64::from(std::process::id()) << 32)
}

/// `builds review sample --landing KEY --repo-dir DIR`: decides whether the
/// landing's review is audited and, when it is, starts the audit in the
/// background. It answers at once and never fails a landing.
fn sample(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &["landing", "repo-dir", "rules-root", "reviewer", "roll"],
        &[],
    )?;
    args.exactly(0, USAGE)?;
    let key = args.require("landing")?;
    let store = store_of(tool, &args)?;
    let root = rules_root(&args)?;
    let Some(conn) = db::open_reader(&store.database())? else {
        return Ok(());
    };
    let Some((landing, true)) = record::latest_landing(&conn, key)? else {
        println!("no passed review of {key}: no audit");
        return Ok(());
    };
    let roll = match args.value("roll")? {
        Some(roll) => roll
            .parse::<f64>()
            .map_err(|_| Fail::Message(format!("'{roll}' is not a number")))?,
        None => (clock_bits() % 1_000_000) as f64 / 1_000_000.0,
    };
    if !audit::sampled(&conn, landing, &group_names(&root)?, roll)? {
        println!("landing {landing} ({key}) is not audited");
        return Ok(());
    }
    let program = std::env::current_exe().context(|| "finding this program".to_string())?;
    let mut command = std::process::Command::new(program);
    command
        .args(["review", "audit", &landing.to_string()])
        .args(["--project", &store.project])
        .args(["--repo-dir", args.require("repo-dir")?])
        .args(["--rules-root", &root.display().to_string()])
        .args(["--reviewer", args.value("reviewer")?.unwrap_or("claude")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
        .spawn()
        .context(|| "starting the audit".to_string())?;
    println!("landing {landing} ({key}) is audited in the background");
    Ok(())
}

/// `builds review misses`: the candidate misses nobody has settled.
fn misses(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, USAGE)?;
    let store = store_of(tool, &args)?;
    let Some(conn) = db::open_reader(&store.database())? else {
        fail!("there is no store for project {}", store.project);
    };
    let mut count = 0;
    for landing in record::audited_landings(&conn)? {
        let findings = record::findings_of_landing(&conn, landing)?;
        for finding in audit::candidate_misses(&findings)
            .into_iter()
            .filter(|f| f.settled.is_none())
        {
            println!("landing {landing}:");
            print!("{}", render::finding_text(finding));
            count += 1;
        }
    }
    println!("{} unsettled", render::plural(count, "candidate miss"));
    Ok(())
}

/// `builds review rates`: each group's audited miss rate.
fn rates(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["rules-root"], &[])?;
    args.exactly(0, USAGE)?;
    let store = store_of(tool, &args)?;
    let root = rules_root(&args)?;
    let Some(conn) = db::open_reader(&store.database())? else {
        fail!("there is no store for project {}", store.project);
    };
    for rate in audit::rates(&conn, &group_names(&root)?)? {
        println!(
            "{}: {} audits, {} confirmed misses; the latest {}: {}{}",
            rate.group,
            rate.audits,
            rate.misses,
            rate.recent_audits,
            rate.recent_misses,
            if rate.risen() { " (risen)" } else { "" }
        );
    }
    Ok(())
}

/// `builds review case add`: the case a confirmed finding or a confirmed miss
/// makes, written into the checkout named by --rules-root (never a default:
/// the main checkout is the person's).
fn case_command(tool: &Tool, args: &[String]) -> Result<()> {
    let Some((sub, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    if sub != "add" {
        fail!("unknown review case command {sub}\n{USAGE}");
    }
    let args = Args::parse(
        rest,
        &[
            "repo-dir",
            "rules-root",
            "name",
            "rule",
            "file",
            "commits",
            "clean-for",
        ],
        &[],
    )?;
    let Some(root) = args.value("rules-root")? else {
        fail!("name the agent-instructions worktree to write the case in with --rules-root");
    };
    let repo_dir = PathBuf::from(args.require("repo-dir")?);
    let patch_of = |base: &str, head: &str| -> Result<String> {
        let output = git(&repo_dir)
            .args(["format-patch", "--stdout", &format!("{base}..{head}")])
            .output()
            .context(|| "running git format-patch".to_string())?;
        if !output.status.success() {
            fail!(
                "git format-patch {base}..{head} failed in {}: {}",
                repo_dir.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let (project, patch, addition, default_name) = match args.positional() {
        [landing, finding] => {
            let cwd = current_dir()?;
            let store = Store::resolve(&tool.places, &args, &cwd)?;
            let Some(conn) = db::open_reader(&store.database())? else {
                fail!("there is no store for project {}", store.project);
            };
            let landing: i64 = landing
                .parse()
                .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))?;
            let finding: i64 = finding
                .parse()
                .map_err(|_| Fail::Message(format!("'{finding}' is not a finding number")))?;
            let found = record::finding_of(&conn, finding)?;
            if found.landing_id != Some(landing) {
                fail!("finding {finding} is not in landing {landing}");
            }
            if !matches!(&found.settled, Some((how, _)) if how == "confirmed") {
                fail!(
                    "finding {finding} is not settled as confirmed; only a confirmed finding becomes a case"
                );
            }
            let run = record::runs_of_landing(&conn, landing)?
                .into_iter()
                .find(|r| r.id == found.run_id)
                .expect("a finding's run belongs to its landing");
            (
                store.project.clone(),
                patch_of(&run.base, &run.head)?,
                (found.rule, Some(found.file)),
                format!("{}-{}", short(&run.head), found.group.replace(' ', "-")),
            )
        }
        [] => {
            let commits = args.require("commits")?;
            let Some((base, head)) = commits.split_once("..") else {
                fail!("--commits is BASE..HEAD");
            };
            let (rule, file, word) = match args.value("clean-for")? {
                Some(rule) => (rule.to_string(), None, "clean"),
                None => (
                    args.require("rule")?.to_string(),
                    Some(args.require("file")?.to_string()),
                    "miss",
                ),
            };
            (
                args.require("project")?.to_string(),
                patch_of(base, head)?,
                (rule, file),
                format!("{}-{word}", short(head)),
            )
        }
        _ => fail!("{USAGE}"),
    };
    let name = args
        .value("name")?
        .map(str::to_string)
        .unwrap_or(default_name);
    let (rule, file) = addition;
    let addition = match &file {
        Some(file) => cases::Addition::Finding { rule: &rule, file },
        None => cases::Addition::Clean { rule: &rule },
    };
    let dir = cases::add(Path::new(root), &name, &project, &patch, addition)?;
    println!("case {name} written in {}; commit it", dir.display());
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
