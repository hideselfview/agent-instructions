//! `builds design places | only | record | amend | approve | check | landed |
//! show | list | render`.
//!
//! The commands that read git (`places`, `record`, `amend`, `check`,
//! `landed`) find the project from the repo of the worktree they are given,
//! or `--project`; git runs before the store's lock is taken.

use std::collections::BTreeSet;
use std::path::Path;

use crate::checkout::Checkout;
use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::args::{Args, read_request};
use crate::lists::design::{self, DesignRequest, Drawn, Landed, PlaceAnswer};
use crate::lists::design_paths::{self, Changes, Globs, PlaceKind};
use crate::lists::store::{self, Store, project_of};
use crate::lists::{Id, Lists, Stamp};
use crate::text::local_minute;
use crate::tool::Tool;

const USAGE: &str = "usage: builds design places DIR SHA...\n       builds design only DIR SHA...\n       builds design record --request FILE\n       builds design amend N --request FILE\n       builds design approve N --by-person\n       builds design check N DIR SHA...\n       builds design landed N DIR SHA...\n       builds design show N\n       builds design list\n       builds design render DIR BOARD [--theme light|dark]";

pub fn cmd_design(tool: &Tool, args: &[String]) -> Result<()> {
    print!("{}", run_to_text(tool, args)?);
    Ok(())
}

/// A design command's output, which `cmd_design` prints.
pub fn run_to_text(tool: &Tool, args: &[String]) -> Result<String> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "places" => places(tool, rest),
        "only" => only(tool, rest),
        "record" => record(tool, rest, &cwd),
        "amend" => amend(tool, rest, &cwd),
        "approve" => approve(tool, rest, &cwd),
        "check" => check(tool, rest),
        "landed" => landed(tool, rest, &cwd),
        "show" => show(tool, rest, &cwd),
        "list" => list(tool, rest, &cwd),
        "render" => render(rest),
        other => fail!("unknown design command {other}\n{USAGE}"),
    }
}

/// What the commits of a worktree change, with the checkout and the store
/// they belong to.
struct Scope {
    checkout: Checkout,
    store: Store,
    lists: Lists,
    globs: Globs,
    /// The last of the commits: what the checkout's files are looked up in.
    tip: String,
    changes: Changes,
}

/// The store of <checkout>'s project (`--project` overrides) and its state.
fn open_store(tool: &Tool, args: &Args, checkout: &Checkout) -> Result<(Store, Lists)> {
    let store = match args.value("project")? {
        Some(project) => Store::named(&tool.places, project)?,
        None => match project_of(&tool.places, checkout)? {
            Some(project) => Store::named(&tool.places, &project)?,
            None => fail!(
                "repo {} is in no project; say which with --project",
                checkout.repo()
            ),
        },
    };
    let lists = store.load()?;
    if !lists.repos.contains(&checkout.repo()) {
        fail!(
            "repo {} is not one of project {}'s ({})",
            checkout.repo(),
            lists.project,
            lists.repos.join(", ")
        );
    }
    Ok((store, lists))
}

fn scope(tool: &Tool, args: &Args, dir: &str, shas: Vec<String>) -> Result<Scope> {
    let checkout = Checkout::find(dir)?;
    let (store, lists) = open_store(tool, args, &checkout)?;
    let globs = Globs::of(&lists, &checkout.repo());
    let changes = design_paths::changes_of(&checkout.top, &globs, &shas)?;
    let tip = shas.last().cloned().unwrap_or_default();
    Ok(Scope {
        checkout,
        store,
        lists,
        globs,
        tip,
        changes,
    })
}

fn lines(paths: impl IntoIterator<Item = String>) -> String {
    paths.into_iter().map(|p| format!("{p}\n")).collect()
}

/// Every design path the commits change, one per line: `place <path>` (a
/// component folder once), `source <path>` (a shared design-system source),
/// `spec <path>#<heading>` (a spec section, when the commits are a design
/// change) or `shared <path>`; nothing when none.
fn places(tool: &Tool, args: &[String]) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [dir, shas @ ..] = args.positional() else {
        fail!("{USAGE}")
    };
    if shas.is_empty() {
        fail!("{USAGE}");
    }
    let scope = scope(tool, &args, dir, shas.to_vec())?;
    let c = scope.changes;
    let places = c.places.into_iter().map(|p| format!("place {p}"));
    let sources = c.sources.into_iter().map(|p| format!("source {p}"));
    let spec = c.spec.into_iter().map(|p| format!("spec {p}"));
    let shared = c.shared.into_iter().map(|p| format!("shared {p}"));
    Ok(lines(places.chain(sources).chain(spec).chain(shared)))
}

/// `only DIR SHA...`: prints nothing and succeeds when every path the
/// commits change is a design file (boards, the design system, spec lines),
/// so they carry no app work; otherwise names the paths that are not.
fn only(tool: &Tool, args: &[String]) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [dir, shas @ ..] = args.positional() else {
        fail!("{USAGE}")
    };
    if shas.is_empty() {
        fail!("{USAGE}");
    }
    let scope = scope(tool, &args, dir, shas.to_vec())?;
    if !scope.changes.is_design_change() {
        fail!("the commits change no design files");
    }
    if !scope.changes.only_design() {
        let outside: Vec<&str> = scope.changes.outside.iter().map(String::as_str).collect();
        fail!(
            "the commits also change files that are no design file: {}",
            outside.join(", ")
        );
    }
    Ok(String::new())
}

/// A design request's design change repo, commits and changes.
fn request_scope(tool: &Tool, args: &Args, request: &DesignRequest) -> Result<Scope> {
    let checkout = Checkout::find(&request.worktree)?;
    let shas = design_paths::commits_of_range(&checkout.top, &request.range)?;
    scope(tool, args, &request.worktree, shas)
}

/// The request's places that are no place the range changes but name a
/// component folder that exists at the range's tip: the components a changed
/// shared source changes the drawing of. A name that is neither is left for
/// the record to refuse as stray.
fn affected_components(scope: &Scope, request: &DesignRequest) -> Result<BTreeSet<String>> {
    let changed = scope.changes.resolvable();
    let mut affected = BTreeSet::new();
    for path in request.places.keys().filter(|p| !changed.contains(*p)) {
        if PlaceKind::of(path) == PlaceKind::Component
            && scope.globs.names_place(path)
            && design_paths::exists_in(&scope.checkout.top, &scope.tip, path.trim_end_matches('/'))?
        {
            affected.insert(path.clone());
        }
    }
    Ok(affected)
}

/// Every board on the request's review page is one of the repo's board
/// places and a file in the worktree, where the dashboard renders it from.
fn require_review_boards(scope: &Scope, request: &DesignRequest) -> Result<()> {
    for section in request.review.iter().flatten() {
        for board in &section.boards {
            if PlaceKind::of(&board.board) != PlaceKind::Board {
                fail!("`review` names {}, which is no board file", board.board);
            }
            if !scope.globs.names_place(&board.board) {
                fail!(
                    "`review` names {}, which is no board place of the repo's design places",
                    board.board
                );
            }
            if !scope.checkout.top.join(&board.board).is_file() {
                fail!(
                    "`review` names {}, which is no file in {}",
                    board.board,
                    scope.checkout.top.display()
                );
            }
        }
    }
    Ok(())
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

/// What a record or amend printed: the change and the items waiting on it,
/// and that the change waits on the person's review.
fn recorded_words(verb: &str, recorded: &design::Recorded, title: &str, cleared: bool) -> String {
    let mut out = format!("{verb} design change {} \"{title}\"\n", recorded.design);
    if cleared {
        out.push_str(
            "the person's approval is cleared: they approved what was drawn before this\n",
        );
    }
    out.push_str("it waits on the person's review (the Waiting on you tab); land.sh --design refuses it until `builds design approve --by-person`\n");
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
    require_review_boards(&scope, &request)?;
    let affected = affected_components(&scope, &request)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let repo = scope.checkout.repo();
    let worktree = scope.checkout.top.display().to_string();
    let words = format!("design record {}", request.title.as_deref().unwrap_or(""));
    let (recorded, title) = scope.store.write(&stamp, &words, |db| {
        let recorded = design::record(
            db,
            &Drawn {
                repo: &repo,
                worktree: &worktree,
            },
            &request,
            &scope.changes,
            &affected,
            &stamp,
        )?;
        let title = store::read(db)?
            .design(recorded.design)
            .expect("just recorded")
            .title
            .clone();
        Ok((recorded, title))
    })?;
    Ok(recorded_words("recorded", &recorded, &title, false))
}

fn amend(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &["request"], &[])?;
    let [id] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let id = Id::parse(id)?;
    let request: DesignRequest = read_request(args.require("request")?)?;
    let scope = request_scope(tool, &args, &request)?;
    require_briefs(&scope, &request)?;
    require_review_boards(&scope, &request)?;
    let affected = affected_components(&scope, &request)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let repo = scope.checkout.repo();
    let worktree = scope.checkout.top.display().to_string();
    let (recorded, title, cleared) =
        scope
            .store
            .write(&stamp, &format!("design amend {id}"), |db| {
                let was_approved = store::read(db)?
                    .design(id)
                    .is_some_and(|d| d.approved.is_some());
                let recorded = design::amend(
                    db,
                    id,
                    &Drawn {
                        repo: &repo,
                        worktree: &worktree,
                    },
                    &request,
                    &scope.changes,
                    &affected,
                    &stamp,
                )?;
                let title = store::read(db)?.design(id).expect("open").title.clone();
                Ok((recorded, title, was_approved))
            })?;
    Ok(recorded_words("amended", &recorded, &title, cleared))
}

/// `approve N --by-person`: the person's review of the boards, said by
/// them. The same approval again writes nothing.
fn approve(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &["by-person"])?;
    let [id] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let id = Id::parse(id)?;
    if !args.switch("by-person") {
        fail!(
            "approving a design change is the person's: pass --by-person once they have said to land design change {id}, never on your own judgment"
        );
    }
    let store = Store::resolve(&tool.places, &args, cwd)?;
    if let Some(approved) = store.load()?.design(id).and_then(|d| d.approved.as_ref()) {
        return Ok(format!(
            "design change {id} was already approved ({})\n",
            local_minute(approved.at)
        ));
    }
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("design approve {id}"), |db| {
        design::approve(db, id, &stamp)
    })?;
    Ok(format!(
        "design change {id} approved; land.sh --design {} can land it\n",
        id.0
    ))
}

/// `check N DIR SHA...`: the store is read, not written.
fn check(tool: &Tool, args: &[String]) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [id, dir, shas @ ..] = args.positional() else {
        fail!("{USAGE}")
    };
    if shas.is_empty() {
        fail!("{USAGE}");
    }
    let id = Id::parse(id)?;
    let scope = scope(tool, &args, dir, shas.to_vec())?;
    design::check_changes(&scope.lists, id, &scope.checkout.repo(), &scope.changes)?;
    Ok(format!(
        "design change {id} resolves every place the commits change and is approved\n"
    ))
}

/// `landed N DIR SHA...`: SHAs are on origin/main, as land.sh calls it after
/// the push; they are stored as full hashes.
fn landed(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [id, dir, shas @ ..] = args.positional() else {
        fail!("{USAGE}")
    };
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
            fail!(
                "commit {sha} is not on origin/main, so design change {id} has not landed with it"
            );
        }
        commits.push(full);
    }
    let repo = checkout.repo();
    // The same commits again write nothing: the version and the log stay.
    if design::already_landed(&lists, id, &repo, &commits)? {
        return Ok(format!(
            "design change {id} was already recorded as landed with these commits\n"
        ));
    }
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    let result = store.write(&stamp, &format!("design landed {id}"), |db| {
        design::landed(db, id, &repo, &commits, &stamp)
    })?;
    Ok(match result {
        Landed::Already => {
            format!("design change {id} was already recorded as landed with these commits\n")
        }
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
    let [id] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let store = Store::resolve(&tool.places, &args, cwd)?;
    design::show(&store.load()?, Id::parse(id)?)
}

fn list(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    args.exactly(0, USAGE)?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    Ok(design::list(&store.load()?))
}

/// `render DIR BOARD [--theme light|dark]`: the board of worktree DIR as one
/// page of HTML, drawn with DIR's design system. BOARD is the board's name
/// (`mac.welcome.open-failed`) or its path (`design/mocks/<name>.dc.html`).
fn render(args: &[String]) -> Result<String> {
    let args = Args::parse(args, &["theme"], &[])?;
    let [dir, board] = args.exactly(2, USAGE)? else {
        unreachable!("exactly two")
    };
    let theme = match args.value("theme")? {
        None | Some("dark") => crate::lists::Theme::Dark,
        Some("light") => crate::lists::Theme::Light,
        Some(other) => fail!("--theme is light or dark, not {other:?}"),
    };
    crate::design_render::render_board(Path::new(dir), board, theme)
}
