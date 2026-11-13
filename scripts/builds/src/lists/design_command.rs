//! `builds design places | record | amend | check | landed | show | list`.
//!
//! The commands that read git (`places`, `record`, `amend`, `check`,
//! `landed`) find the project from the repo of the worktree they are given,
//! or `--project`; git runs before the store's lock is taken.

use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::{Args, read_request};
use crate::lists::design::{self, DesignRequest, Landed, PlaceAnswer};
use crate::lists::design_paths::{self, Changes};
use crate::lists::store::{Store, project_of};
use crate::lists::{Id, Lists, Stamp};
use crate::tool::Tool;

const USAGE: &str = "usage: builds design places DIR SHA...\n       builds design record --request FILE\n       builds design amend N --request FILE\n       builds design check N DIR SHA...\n       builds design landed N DIR SHA...\n       builds design show N\n       builds design list";

pub fn cmd_design(tool: &Tool, args: &[String]) -> Result<()> {
    print!("{}", run_to_text(tool, args)?);
    Ok(())
}

/// A design command's output, which `cmd_design` prints.
pub fn run_to_text(tool: &Tool, args: &[String]) -> Result<String> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    let Some((command, rest)) = args.split_first() else { fail!("{USAGE}") };
    match command.as_str() {
        "places" => places(tool, rest),
        "record" => record(tool, rest, &cwd),
        "amend" => amend(tool, rest, &cwd),
        "check" => check(tool, rest),
        "landed" => landed(tool, rest, &cwd),
        "show" => show(tool, rest, &cwd),
        "list" => list(tool, rest, &cwd),
        other => fail!("unknown design command {other}\n{USAGE}"),
    }
}

/// What the commits of a worktree change, with the checkout and the store
/// they belong to.
struct Scope {
    checkout: Checkout,
    store: Store,
    lists: Lists,
    changes: Changes,
}

/// The store of <checkout>'s project (`--project` overrides) and its state.
fn open_store(tool: &Tool, args: &Args, checkout: &Checkout) -> Result<(Store, Lists)> {
    let store = match args.value("project")? {
        Some(project) => Store::named(&tool.places, project)?,
        None => match project_of(&tool.places, checkout)? {
            Some(project) => Store::named(&tool.places, &project)?,
            None => fail!("repo {} is in no project; say which with --project", checkout.repo()),
        },
    };
    let lists = store.load()?;
    if !lists.repos.contains(&checkout.repo()) {
        fail!("repo {} is not one of project {}'s ({})", checkout.repo(), lists.project, lists.repos.join(", "));
    }
    Ok((store, lists))
}

fn scope(tool: &Tool, args: &Args, dir: &str, shas: Vec<String>) -> Result<Scope> {
    let checkout = Checkout::find(dir)?;
    let (store, lists) = open_store(tool, args, &checkout)?;
    let globs = lists.design_places.get(&checkout.repo()).cloned().unwrap_or_default();
    let changes = design_paths::changes_of(&checkout.top, &globs, &shas)?;
    Ok(Scope { checkout, store, lists, changes })
}

fn lines(paths: impl IntoIterator<Item = String>) -> String {
    paths.into_iter().map(|p| format!("{p}\n")).collect()
}

/// The design places the commits change, one per line; nothing when none.
fn places(tool: &Tool, args: &[String]) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [dir, shas @ ..] = args.positional() else { fail!("{USAGE}") };
    if shas.is_empty() {
        fail!("{USAGE}");
    }
    let scope = scope(tool, &args, dir, shas.to_vec())?;
    Ok(lines(scope.changes.places))
}

/// A design request's design change repo, commits and changes.
fn request_scope(tool: &Tool, args: &Args, request: &DesignRequest) -> Result<Scope> {
    let checkout = Checkout::find(&request.worktree)?;
    let shas = design_paths::commits_of_range(&checkout.top, &request.range)?;
    scope(tool, args, &request.worktree, shas)
}

/// The briefs of new items are looked for under the main checkout.
fn require_briefs(scope: &Scope, request: &DesignRequest) -> Result<()> {
    for answer in request.places.values() {
        if let PlaceAnswer::New(draft) = answer {
            draft.require_brief(&scope.checkout.main)?;
        }
    }
    Ok(())
}

/// What a record or amend printed: the change and the items waiting on it.
fn recorded_words(verb: &str, recorded: &design::Recorded, title: &str) -> String {
    let mut out = format!("{verb} design change {} \"{title}\"\n", recorded.design);
    if !recorded.new_items.is_empty() {
        let ids: Vec<String> = recorded.new_items.iter().map(|id| id.to_string()).collect();
        out.push_str(&format!("new items {} wait on it\n", ids.join(", ")));
    }
    out
}

fn record(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &["request"], &[])?;
    args.exactly(0, USAGE)?;
    let request: DesignRequest = read_request(args.require("request")?)?;
    let scope = request_scope(tool, &args, &request)?;
    require_briefs(&scope, &request)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let repo = scope.checkout.repo();
    let words = format!("design record {}", request.title.as_deref().unwrap_or(""));
    let (recorded, title) = scope.store.with_write(&stamp, &words, |lists| {
        let recorded = design::record(lists, &repo, &request, &scope.changes, &stamp)?;
        let title = lists.design(recorded.design).expect("just recorded").title.clone();
        Ok((recorded, title))
    })?;
    Ok(recorded_words("recorded", &recorded, &title))
}

fn amend(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &["request"], &[])?;
    let [id] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let id = Id::parse(id)?;
    let request: DesignRequest = read_request(args.require("request")?)?;
    let scope = request_scope(tool, &args, &request)?;
    require_briefs(&scope, &request)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let repo = scope.checkout.repo();
    let (recorded, title) = scope.store.with_write(&stamp, &format!("design amend {id}"), |lists| {
        let recorded = design::amend(lists, id, &repo, &request, &scope.changes, &stamp)?;
        let title = lists.design(id).expect("open").title.clone();
        Ok((recorded, title))
    })?;
    Ok(recorded_words("amended", &recorded, &title))
}

/// `check N DIR SHA...`: the store is read, not written.
fn check(tool: &Tool, args: &[String]) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [id, dir, shas @ ..] = args.positional() else { fail!("{USAGE}") };
    if shas.is_empty() {
        fail!("{USAGE}");
    }
    let id = Id::parse(id)?;
    let scope = scope(tool, &args, dir, shas.to_vec())?;
    design::check_changes(&scope.lists, id, &scope.checkout.repo(), &scope.changes)?;
    Ok(format!("design change {id} resolves every place the commits change\n"))
}

/// `landed N DIR SHA...`: SHAs are on origin/main, as land.sh calls it after
/// the push; they are stored as full hashes.
fn landed(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [id, dir, shas @ ..] = args.positional() else { fail!("{USAGE}") };
    if shas.is_empty() {
        fail!("{USAGE}");
    }
    let id = Id::parse(id)?;
    let checkout = Checkout::find(dir)?;
    let (store, lists) = open_store(tool, &args, &checkout)?;
    let mut commits = Vec::new();
    for sha in shas {
        let full = design_paths::full_sha(&checkout.top, sha)?;
        if !design_paths::on_origin_main(&checkout.top, &full)? {
            fail!("commit {sha} is not on origin/main, so design change {id} has not landed with it");
        }
        commits.push(full);
    }
    let repo = checkout.repo();
    // The same commits again write nothing: the version and the log stay.
    if design::already_landed(&lists, id, &repo, &commits)? {
        return Ok(format!("design change {id} was already recorded as landed with these commits\n"));
    }
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let result = store.with_write(&stamp, &format!("design landed {id}"), |lists| {
        design::landed(lists, id, &repo, &commits, &stamp)
    })?;
    Ok(match result {
        Landed::Already => format!("design change {id} was already recorded as landed with these commits\n"),
        Landed::Closed { ready } => {
            let mut out = format!("design change {id} landed\n");
            for (item, title) in ready {
                out.push_str(&format!("item {item} \"{title}\" is ready\n"));
            }
            out
        }
    })
}

fn show(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [id] = args.exactly(1, USAGE)? else { unreachable!("exactly one") };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    design::show(&store.load()?, Id::parse(id)?)
}

fn list(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, USAGE)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    Ok(design::list(&store.load()?))
}
