//! `builds review`: reviews a change against the rules of agent-instructions,
//! one reviewer per group of rules (one folder of `rules/` each), and records
//! every review in the project's store.
//!
//! ```text
//! builds review [run] WORKTREE [BASE] [--landing KEY] [--item N]... [--head SHA] [--rules-root DIR] [--reviewer PROGRAM]
//! builds review settle FINDING confirmed|dropped --why TEXT
//! builds review settle FINDING --again --why TEXT
//! builds review fixed FINDING SHA
//! builds review show LANDING
//! builds review check-landing | landed DIR --project P SHA...
//! builds review groups [--rules-root DIR] [--project P] [--changed PATH]...
//! builds review replay [--case NAME]... [--source REPO] [--model M] [--rules-root DIR] [--reviewer PROGRAM]
//! builds review replay --landing N... --repo-dir DIR [--group NAME]... [--model M] [--rules-root DIR] [--reviewer PROGRAM]
//! builds review audit LANDING --repo-dir DIR
//! builds review misses | rates | scores
//! builds review case add ...
//! ```
//!
//! `run` reviews the worktree's net change past BASE (default `origin/main`)
//! before it lands. No diff is reviewed more than once. Each change gets one
//! review; the coordinator reads the fixes for its confirmed findings. The
//! groups whose rules cover a changed path each get one `claude -p`. The run
//! records the net change it read, with the patch ids of its commits, under
//! the change's KEY, then says what `builds land` would say of the branch's
//! commits (`landing.rs`), and exits 1 unless land would take them. A KEY
//! reviewed already gets no second review of a diff: the run asks only the
//! groups that gave the review no answer, over the diff they did not answer,
//! and reads in a new run only how the branch's net change now differs from
//! the net change the review read last (`run::diff_since`): the lines the
//! branch adds or removes that it did not then, fix commits included, and the
//! lines it no longer changes, without the commits a rebase brings in from
//! the base. `landing::Coverage` says what the review read, for this run and
//! for `builds land`. The run refuses when the review lacks nothing.
//!
//! Every call records its setup (its group's model and prompt), its tokens and
//! its cost. `scores` counts how each setup did from the settled findings, and
//! `replay --landing` asks again over recorded diffs and counts the replay the
//! same way (`score.rs`).

mod audit;
pub mod call;
mod cases;
pub mod check;
pub mod commits;
#[cfg(test)]
pub mod fakes;
mod files;
pub mod groups;
pub mod landing;
mod prompt;
pub mod record;
mod render;
mod rerun;
pub mod rules;
pub mod run;
mod score;

use std::path::{Path, PathBuf};

use crate::checkout::{Checkout, git};
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::Stamp;
use crate::lists::args::{Args, current_dir};
use crate::lists::store::{Store, project_of, resolve_by};
use crate::tool::Tool;

use record::Kind;

const USAGE: &str = "usage: builds review [run] WORKTREE [BASE] [--landing KEY] [--item N]... [--head SHA] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review settle FINDING confirmed|dropped --why TEXT\n       builds review settle FINDING --again --why TEXT\n       builds review fixed FINDING SHA\n       builds review show LANDING\n       builds review check-landing DIR --project P SHA... [--rules-root DIR]\n       builds review landed DIR --project P SHA... [--rules-root DIR] [--roll N] [--reviewer PROGRAM]\n       builds review groups [--rules-root DIR] [--project P] [--changed PATH]...\n       builds review replay [--case NAME]... [--source REPO] [--model opus|sonnet|haiku] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review replay --landing N... --repo-dir DIR [--group NAME]... [--model opus|sonnet|haiku] [--rules-root DIR] [--reviewer PROGRAM]\n       builds review audit LANDING --repo-dir DIR [--rules-root DIR] [--reviewer PROGRAM]\n       builds review misses\n       builds review rates [--rules-root DIR]\n       builds review scores\n       builds review case add LANDING FINDING --repo-dir DIR --rules-root DIR [--name NAME]\n       builds review case add --rule SLUG --file PATH --commits BASE..HEAD --project P --name NAME --repo-dir DIR --rules-root DIR\n       builds review case add --clean-for SLUG --commits BASE..HEAD --project P --name NAME --repo-dir DIR --rules-root DIR";

/// The checkout the rules and groups are read from: `--rules-root`, else the
/// one `install.sh` links at ~/.codex/agent-instructions.
fn rules_root(args: &Args) -> Result<PathBuf> {
    match args.value("rules-root")? {
        Some(root) => Ok(PathBuf::from(root)),
        None => default_rules_root(),
    }
}

/// The checkout `install.sh` links at ~/.codex/agent-instructions.
pub fn default_rules_root() -> Result<PathBuf> {
    match std::env::var_os("HOME") {
        Some(home) => Ok(PathBuf::from(home).join(".codex/agent-instructions")),
        None => fail!("HOME is not set; name the checkout with --rules-root"),
    }
}

/// The file a review of the commits up to <head> holds locked while it runs,
/// in the store's folder; the dashboard reads a held one as a review running.
pub fn running_file(store_dir: &Path, head: &str) -> PathBuf {
    store_dir.join("reviews-running").join(head)
}

/// Whether a review of the commits up to <head> runs now.
pub fn is_running(store_dir: &Path, head: &str) -> bool {
    crate::queue::is_held(&running_file(store_dir, head)).unwrap_or(false)
}

/// Holds <path> locked until dropped, then removes it.
struct Running {
    path: PathBuf,
    fd: libc::c_int,
}

impl Running {
    fn start(path: PathBuf) -> Result<Running> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
        }
        match crate::queue::open_with(&path, libc::O_CREAT | libc::O_NONBLOCK | libc::O_CLOEXEC) {
            Ok(fd) => Ok(Running { path, fd }),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                fail!("a review of these commits runs already")
            }
            Err(error) => fail!("locking {}: {error}", path.display()),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        unsafe { libc::close(self.fd) };
    }
}

pub fn cmd_review(tool: &Tool, args: &[String]) -> Result<()> {
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "groups" => list_groups(rest),
        "settle" => settle(tool, rest),
        "fixed" => fixed(tool, rest),
        "show" => show(tool, rest),
        "run" => run_command(tool, rest),
        "check-landing" => landing::check_landing_command(tool, rest),
        "landed" => landing::landed_command(tool, rest),
        "replay" => replay(tool, rest),
        "audit" => audit_command(tool, rest),
        "misses" => misses(tool, rest),
        "rates" => rates(tool, rest),
        "scores" => scores(tool, rest),
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
    /// Names the change, and so its landing under review.
    pub key: &'a str,
    pub root: &'a Path,
    pub reviewer: &'a Path,
    pub stamp: &'a Stamp,
    /// (path, text) of the landing's brief.
    pub brief: Option<(String, String)>,
}

/// What a review found.
#[derive(Debug)]
pub struct Reported {
    pub landing: Option<i64>,
    /// Everything to print.
    pub text: String,
    /// What `builds land` would refuse these commits for; None when it
    /// would take them.
    pub refusal: Option<String>,
}

/// Reviews the worktree's net change past the base and records it. A change
/// gets one review: when its key has one already, this asks only what the
/// review lacks (`later_asks`), and refuses when it lacks nothing.
pub fn review(inputs: &Inputs) -> Result<Reported> {
    let conn = inputs.store.open_existing()?;
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
    let commits = commits::with_patch_ids(
        inputs.worktree,
        &commits::in_range(inputs.worktree, &diff.base, &diff.head)?,
    )?;
    let material = run::Material::load(inputs.root, inputs.project)?;
    if material.group_names(&diff.changed).is_empty() {
        return Ok(Reported {
            landing: None,
            text: "no group's rules cover the changed paths: nothing to review, and builds land needs no review\n".to_string(),
            refusal: None,
        });
    }
    let open = record::open_landing(&conn, inputs.key)?;
    let asks = match open {
        None => vec![Ask {
            groups: material.group_names(&diff.changed),
            diff,
            since: None,
            commits: commits.clone(),
            note: None,
        }],
        Some(landing) => later_asks(&conn, inputs, &material, &diff, &commits, landing)?,
    };
    let mut running: Vec<(String, Running)> = Vec::new();
    let mut answers = Vec::new();
    for ask in &asks {
        if !running.iter().any(|(head, _)| *head == ask.diff.head) {
            let file = running_file(&inputs.store.dir, &ask.diff.head);
            running.push((ask.diff.head.clone(), Running::start(file)?));
        }
        let picked: Vec<_> = material
            .groups
            .pick(&material.rules, &ask.diff.changed)
            .into_iter()
            .filter(|(group, _)| ask.groups.contains(&group.name))
            .collect();
        answers.push(run::ask_groups(
            &material,
            inputs.reviewer,
            &picked,
            &ask.diff,
            inputs.brief.as_ref(),
        ));
    }
    let stamp = inputs.stamp;
    let (landing, runs) = inputs
        .store
        .write(stamp, &format!("review {}", inputs.key), |db| {
            let landing = match open {
                Some(id) => id,
                None => record::add_landing(db, inputs.key, inputs.repo, stamp.at)?,
            };
            let mut runs = Vec::new();
            for (ask, asked) in asks.iter().zip(&answers) {
                let (run_id, _) = record::add_run(
                    db,
                    &record::NewRun {
                        landing: Some(landing),
                        kind: Kind::Review,
                        label: None,
                        at: stamp.at,
                        base: &ask.diff.base,
                        head: &ask.diff.head,
                        since: ask.since,
                        commits: &ask.commits,
                        grouping: &material.grouping,
                        rules_version: &run::rules_version(inputs.root),
                    },
                    &asked.calls,
                )?;
                runs.push((
                    run_id,
                    record::calls_of_run(db, run_id)?,
                    record::findings_of_run(db, run_id)?,
                ));
            }
            Ok((landing, runs))
        })?;
    drop(running);
    let mut text = String::new();
    for ((ask, asked), (run_id, calls, findings)) in asks.iter().zip(&answers).zip(&runs) {
        text.push_str(&format!(
            "review of {} (landing {landing}, run {run_id}) over {}...{}\n",
            inputs.key,
            short(&ask.diff.base),
            short(&ask.diff.head)
        ));
        if let Some(note) = &ask.note {
            text.push_str(note);
        }
        for group in &asked.nothing_to_read {
            text.push_str(&format!(
                "{group}: the diff holds none of the files its rules cover (a commit changed one and a later commit changed it back), so it had nothing to read: its answer is no findings\n"
            ));
        }
        text.push_str(&render::run_report(calls, findings));
    }
    // What builds land would say of the branch's commits now.
    let needs = landing::needs(inputs.worktree, commits.clone(), |paths| {
        material.group_names(paths)
    })?;
    let verdict = landing::judge(
        &conn,
        inputs.repo,
        inputs.worktree,
        &needs,
        landing::CodeCheck::Branch(inputs.head.unwrap_or("HEAD")),
        &|paths| material.group_names(paths),
    )?;
    text.push_str(&landing::record_code_left(inputs.store, &verdict, || {
        Ok(stamp.clone())
    })?);
    let refusal = landing::refusal(&commits, &verdict);
    if refusal.is_none() {
        text.push_str(&format!("builds land: {}\n", landing::taken(&verdict)));
    }
    Ok(Reported {
        landing: Some(landing),
        text,
        refusal,
    })
}

/// One run of a review: the diff its reviewers read, the commits of the net
/// change it read, and the groups it asks.
struct Ask {
    diff: run::Diff,
    /// The earlier run whose net change the diff is the difference from.
    since: Option<i64>,
    commits: Vec<commits::Commit>,
    groups: Vec<String>,
    /// What the run asked, for a run of a change that has a review already.
    note: Option<String>,
}

/// The runs a change whose review <landing> holds gets now, so that the
/// review reads <diff>, the branch's net change of <commits>: for each diff a
/// run read, the groups asked over it that gave no answer over it, asked over
/// that diff again; and, when the net change differs from the net change the
/// review read last on a path a group covers, the groups that cover a path
/// where it differs, asked over that difference (`run::diff_since`). No
/// reviewer reads a diff twice. Fails with why builds review reviews no change
/// twice when the review lacks nothing.
fn later_asks(
    conn: &rusqlite::Connection,
    inputs: &Inputs,
    material: &run::Material,
    diff: &run::Diff,
    commits: &[commits::Commit],
    landing: i64,
) -> Result<Vec<Ask>> {
    let coverage = landing::Coverage::of(conn, landing)?;
    let calls = record::calls_of_landing(conn, landing)?;
    let read: Vec<&record::RunRow> = coverage.runs.iter().map(|(run, _)| run).collect();
    let mut asks = Vec::new();
    for (run, missing) in coverage.unanswered(&calls) {
        asks.push(Ask {
            diff: run::diff_of_run(inputs.worktree, &read, run, |run| {
                Ok((run.base.clone(), run.head.clone()))
            })?,
            since: run.since,
            commits: coverage.commits_of(run.id)?.to_vec(),
            note: Some(format!(
                "asked only the groups that gave run {} no answer, over the diff it read: {}\n",
                run.id,
                missing.join(", ")
            )),
            groups: missing,
        });
    }
    let last = coverage
        .last_read()
        .expect("builds review opens every landing with its review run");
    let mut since = run::diff_since(
        inputs.worktree,
        &last.base,
        &last.head,
        &diff.base,
        &diff.head,
    )?;
    if let (Some(source), Some(read)) = (&mut since.source, &diff.source) {
        source.in_place = read.in_place;
    }
    let groups = material.group_names(&since.changed);
    if !groups.is_empty() {
        asks.push(Ask {
            note: Some(format!(
                "read only how the branch's net change differs from the net change run {} read over {}...{}\n",
                last.id,
                short(&last.base),
                short(&last.head)
            )),
            diff: since,
            since: Some(last.id),
            commits: commits.to_vec(),
            groups,
        });
    }
    if asks.is_empty() {
        fail!(
            "{}\nrun {} of review landing {landing} ({}) read the branch's net change over {}...{}, every group answered, and the branch's net change differs from it on no path a group's rules cover, so builds review does not review it again.\nbuilds review show {landing} lists the review and its findings.",
            landing::ONE_REVIEW,
            last.id,
            inputs.key,
            short(&last.base),
            short(&last.head)
        );
    }
    Ok(asks)
}

fn short(rev: &str) -> &str {
    &rev[..rev.len().min(10)]
}

fn run_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &["landing", "item", "rules-root", "reviewer", "head"],
        &[],
    )?;
    let reported = run_review(tool, &args)?;
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

/// Runs the review `builds review run` names and returns what it found.
fn run_review(tool: &Tool, args: &Args) -> Result<Reported> {
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
    review(&Inputs {
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
    })
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

/// `builds review settle FINDING confirmed|dropped --why TEXT`, or
/// `builds review settle FINDING --again --why TEXT`, which changes a
/// confirmed finding that names no fix to dropped and keeps its first settle.
fn settle(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["why"], &["again"])?;
    let again = args.switch("again");
    let (finding, how) = match (args.positional(), again) {
        ([finding], true) => (finding, "dropped"),
        ([finding, how], false) => (finding, how.as_str()),
        _ => fail!("{USAGE}"),
    };
    let id: i64 = finding
        .parse()
        .map_err(|_| Fail::Message(format!("'{finding}' is not a finding number")))?;
    let why = args.why()?;
    let (store, _) = store_of(tool, &args)?;
    let stamp = Stamp::resolve(tool.now, &args, &current_dir()?)?;
    if again {
        store.write(&stamp, &format!("review settle {id} again"), |db| {
            record::settle_again(db, id, why, stamp.at, &stamp.by)
        })?;
        println!(
            "You settled finding {id} again, as dropped; the finding keeps its first settle beside the second."
        );
        return Ok(());
    }
    store.write(&stamp, &format!("review settle {id}"), |db| {
        record::settle(db, id, how, why, stamp.at, &stamp.by)
    })?;
    println!("finding {id} settled {how}");
    Ok(())
}

/// `builds review fixed FINDING SHA`: names commit SHA, of the repo the
/// command runs in, as the fix of confirmed finding FINDING.
fn fixed(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    let [finding, sha] = args.exactly(2, USAGE)? else {
        unreachable!("exactly two")
    };
    let id: i64 = finding
        .parse()
        .map_err(|_| Fail::Message(format!("'{finding}' is not a finding number")))?;
    let (store, _) = store_of(tool, &args)?;
    let cwd = current_dir()?;
    let Some(checkout) = Checkout::at(&cwd)? else {
        fail!("run builds review fixed in a checkout of the repo that holds {sha}");
    };
    let fix = landing::fix_commit(&checkout.top, sha)?;
    let stamp = Stamp::resolve(tool.now, &args, &cwd)?;
    store.write(&stamp, &format!("review fixed {id}"), |db| {
        record::record_fix(db, id, &fix, stamp.at, &stamp.by)
    })?;
    println!(
        "finding {id} is fixed by {}. Read it against the finding; the next builds review reads it as part of the branch's net change",
        short(&fix.sha)
    );
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
    let (_, conn) = store_of(tool, &args)?;
    let landing = record::landing(&conn, id)?;
    println!(
        "landing {id}: {} ({}), {}",
        landing.key,
        landing.repo,
        match landing.landed_run {
            Some(run) => format!("landed with run {run}"),
            None => "not landed".to_string(),
        }
    );
    for record::RunRow {
        id: run,
        kind,
        base,
        head,
        since,
        ..
    } in record::runs_of_landing(&conn, id)?
    {
        match since {
            Some(since) => println!(
                "{kind} {run} over {}...{}, read as how it differs from the net change run {since} read",
                short(&base),
                short(&head)
            ),
            None => println!("{kind} {run} over {}...{}", short(&base), short(&head)),
        }
        let read = record::commits_of_run(&conn, run)?;
        if !read.is_empty() {
            let shas: Vec<&str> = read.iter().map(|c| short(&c.sha)).collect();
            println!("  commits: {}", shas.join(" "));
        }
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
/// tokens. Each replayed case is recorded in its project's store. With
/// `--landing`, it replays the diffs of recorded reviews instead (`rerun.rs`).
fn replay(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(
        args,
        &[
            "case",
            "rules-root",
            "reviewer",
            "source",
            "model",
            "landing",
            "repo-dir",
            "group",
        ],
        &[],
    )?;
    args.exactly(0, USAGE)?;
    if let Some(model) = args.value("model")?
        && !["opus", "sonnet", "haiku"].contains(&model)
    {
        fail!("--model is opus, sonnet or haiku, not {model}");
    }
    if !args.values("landing").is_empty() {
        return replay_landings(tool, &args);
    }
    if args.value("repo-dir")?.is_some() || !args.values("group").is_empty() {
        fail!("--repo-dir and --group go with --landing\n{USAGE}");
    }
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
    // replay records each case's run in its project's store, so the command
    // checks that every store exists before any reviewer runs.
    let stores = all
        .iter()
        .map(|case| {
            let store = Store::named(&tool.places, &case.project)?;
            store.open_existing()?;
            Ok(store)
        })
        .collect::<Result<Vec<Store>>>()?;
    let cwd = current_dir()?;
    let stamp = Stamp::resolve(tool.now, &args, &cwd)?;
    let rules_version = run::rules_version(&root);
    let results = cases::replay_all(
        &root,
        &all,
        &reviewer,
        source.as_deref(),
        args.value("model")?,
    )?;
    for ((case, result), store) in all.iter().zip(&results).zip(&stores) {
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

/// `builds review replay --landing N...`: replays the diffs the reviews of
/// those landings read and scores the replay against their settled findings.
fn replay_landings(tool: &Tool, args: &Args) -> Result<()> {
    if !args.values("case").is_empty() || args.value("source")?.is_some() {
        fail!(
            "--landing replays recorded reviews, and --case and --source replay cases: give one or the other"
        );
    }
    let landings: Vec<i64> = args
        .values("landing")
        .into_iter()
        .map(|landing| {
            landing
                .parse()
                .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))
        })
        .collect::<Result<_>>()?;
    // rerun::replay opens the store, and fails when the project has none.
    let store = Store::resolve(&tool.places, args, &current_dir()?)?;
    let root = rules_root(args)?;
    let reviewer = PathBuf::from(args.value("reviewer")?.unwrap_or("claude"));
    let repo_dir = PathBuf::from(args.require("repo-dir")?);
    let stamp = Stamp::resolve(tool.now, args, &current_dir()?)?;
    let replayed = rerun::replay(&rerun::Inputs {
        store: &store,
        project: &store.project,
        repo_dir: &repo_dir,
        landings: &landings,
        root: &root,
        reviewer: &reviewer,
        model: args.value("model")?,
        groups: &args.values("group"),
        stamp: &stamp,
    })?;
    print!("{}", replayed.text);
    if replayed.failed {
        return Err(Fail::Exit(1));
    }
    Ok(())
}

/// The store of a command that names no worktree, with its database open
/// for reading: `--project`, else the project of the current directory's
/// repo. Fails when that project has no store, so that no review command
/// answers from an empty one.
fn store_of(tool: &Tool, args: &Args) -> Result<(Store, crate::db::Held)> {
    let store = Store::resolve(&tool.places, args, &current_dir()?)?;
    let conn = store.open_existing()?;
    Ok((store, conn))
}

/// `rule` mapped to the name of its group in the checkout's group folders.
pub(super) fn group_names(root: &Path) -> Result<impl Fn(&str) -> Option<String>> {
    let groups = groups::Groups::load(root)?;
    Ok(move |rule: &str| {
        groups
            .groups
            .iter()
            .find(|g| g.rules.iter().any(|r| r == rule))
            .map(|g| g.name.clone())
    })
}

/// `builds review audit LANDING --repo-dir DIR`: audits a landing that landed.
fn audit_command(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &["repo-dir", "rules-root", "reviewer", "seed"], &[])?;
    let [landing] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let landing: i64 = landing
        .parse()
        .map_err(|_| Fail::Message(format!("'{landing}' is not a landing number")))?;
    // audit::audit opens the store, and fails when the project has none.
    let store = Store::resolve(&tool.places, &args, &current_dir()?)?;
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
pub(super) fn clock_bits() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1);
    (now as u64) ^ (u64::from(std::process::id()) << 32)
}

/// `builds review misses`: the candidate misses nobody has settled.
fn misses(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, USAGE)?;
    let (_, conn) = store_of(tool, &args)?;
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
    let (_, conn) = store_of(tool, &args)?;
    let root = rules_root(&args)?;
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

/// `builds review scores`: precision and recall per group and rule, for each
/// setup, from the settled findings and audits in the store.
fn scores(tool: &Tool, args: &[String]) -> Result<()> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, USAGE)?;
    let (store, conn) = store_of(tool, &args)?;
    print!(
        "{}",
        score::store_report(&store.project, &score::of_store(&conn)?)
    );
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
            let (store, conn) = store_of(tool, &args)?;
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
            if !found.settled_as("confirmed") {
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
            // builds review replay records this case's run in the project's
            // store, so the project must have a store.
            let store = Store::named(&tool.places, args.require("project")?)?;
            store.open_existing()?;
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
                store.project,
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
