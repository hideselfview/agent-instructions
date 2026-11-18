//! `builds design places | only | record | amend | approve | check | landed |
//! drop | show | list | render | tokens-css | publish-files | publish-last`.
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
use crate::lists::design::{self, DesignRequest, Drawn, Landed, PlaceAnswer, Settlement};
use crate::lists::design_paths::{self, Changes, Globs, PlaceKind};
use crate::lists::design_session::{self, Messaging};
use crate::lists::store::{self, Store, project_of};
use crate::lists::{Id, Lists, Stamp};
use crate::text::local_minute;
use crate::tool::Tool;

const USAGE: &str = "usage: builds design places DIR REV...\n       builds design only DIR REV...\n       builds design record --request FILE\n       builds design amend N --request FILE\n       builds design approve N --by-person\n       builds design check N[,N...] DIR REV...\n       builds design landed N DIR SHA...\n       builds design drop N --why WHY\n       builds design show N\n       builds design list\n       builds design render DIR BOARD [--theme light|dark] [--now | --at REV]\n       builds design tokens-css DIR\n       builds design publish-files DIR OUT [--at REV]\n       builds design publish-last FOLDER --listing FILE [--index FILE --by WHO]";

pub fn cmd_design(tool: &Tool, args: &[String]) -> Result<()> {
    print!("{}", run_to_text(tool, args)?);
    Ok(())
}

/// A design command's output, which `cmd_design` prints.
pub fn run_to_text(tool: &Tool, args: &[String]) -> Result<String> {
    run_in_session(tool, args, Messaging::from_env)
}

/// A design command run by the session whose inbox <session> reads, if any:
/// only `record` and `amend` call it, and keep the inbox as the session to
/// tell, so no other command depends on the environment.
pub fn run_in_session(
    tool: &Tool,
    args: &[String],
    session: impl FnOnce() -> Result<Option<Messaging>>,
) -> Result<String> {
    let cwd = std::env::current_dir().context(|| "reading the current directory".to_string())?;
    let Some((command, rest)) = args.split_first() else {
        fail!("{USAGE}")
    };
    match command.as_str() {
        "places" => places(tool, rest),
        "only" => only(tool, rest),
        "record" => record(tool, rest, &cwd, session()?.as_ref()),
        "amend" => amend(tool, rest, &cwd, session()?.as_ref()),
        "approve" => approve(tool, rest, &cwd),
        "check" => check(tool, rest),
        "landed" => landed(tool, rest),
        "drop" => drop(tool, rest, &cwd),
        "show" => show(tool, rest, &cwd),
        "list" => list(tool, rest, &cwd),
        "render" => render(tool, rest),
        "tokens-css" => tokens_css(rest),
        "publish-files" => publish_files(tool, rest),
        "publish-last" => publish_last(rest),
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

fn scope(tool: &Tool, args: &Args, checkout: Checkout, shas: Vec<String>) -> Result<Scope> {
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

/// The scope of the commits <revs> name in worktree <dir>: each is a commit,
/// or a range (`A~1..A`) that stands for its commits, oldest first.
/// `named_scope` fails on a name that is not a commit, because otherwise the
/// design paths would find no boards in it and count every board as deleted.
fn named_scope(tool: &Tool, args: &Args, dir: &str, revs: &[String]) -> Result<Scope> {
    let checkout = Checkout::find(dir)?;
    let mut shas = Vec::new();
    for rev in revs {
        if rev.contains("..") {
            shas.extend(design_paths::commits_of_range(&checkout.top, rev)?);
        } else {
            shas.push(design_paths::full_sha(&checkout.top, rev)?);
        }
    }
    scope(tool, args, checkout, shas)
}

fn lines(paths: impl IntoIterator<Item = String>) -> String {
    paths.into_iter().map(|p| format!("{p}\n")).collect()
}

/// Every design path the commits change, one per line: `place <path>` (a
/// component folder once), `source <path>` (a shared design-system source),
/// `deleted <path>` (a board file the commits delete, which needs no answer),
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
    let scope = named_scope(tool, &args, dir, shas)?;
    let c = scope.changes;
    let places = c.places.into_iter().map(|p| format!("place {p}"));
    let sources = c.sources.into_iter().map(|p| format!("source {p}"));
    let spec = c.spec.into_iter().map(|p| format!("spec {p}"));
    let shared = c.shared.into_iter().map(|p| format!("shared {p}"));
    let deleted = c.deleted.into_iter().map(|p| format!("deleted {p}"));
    Ok(lines(
        places
            .chain(deleted)
            .chain(sources)
            .chain(spec)
            .chain(shared),
    ))
}

/// `only DIR REV...`: prints nothing and succeeds when every path the
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
    let scope = named_scope(tool, &args, dir, shas)?;
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
    scope(tool, args, checkout, shas)
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
            && boards::exists_in(&scope.checkout.top, &scope.tip, path.trim_end_matches('/'))?
        {
            affected.insert(path.clone());
        }
    }
    Ok(affected)
}

/// Every board on the request's review page is one of the repo's board
/// places and a file at the range's last commit, which the dashboard draws it
/// from, and a board with circles is a file on origin/main too: the page draws
/// a board that is no file there as new, whole, with no circles.
fn require_review_boards(scope: &Scope, request: &DesignRequest) -> Result<()> {
    if let Some(section) = &request.review {
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
            if !boards::exists_in(&scope.checkout.top, &scope.tip, &board.board)? {
                fail!(
                    "`review` names {}, which is no file at the range's last commit {}",
                    board.board,
                    scope.tip
                );
            }
            let now = crate::design_render::NOW;
            if !board.circles.is_empty()
                && !boards::exists_in(&scope.checkout.top, now, &board.board)?
            {
                fail!(
                    "`review` circles elements on {}, which is no file on {now}: a board the change adds is new whole, so leave out its circles",
                    board.board
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

/// Where a recorded or amended change stands with the person's review.
enum Review {
    /// A fix that changes no design, which builds approves when it records
    /// the fix.
    Fix,
    /// A board edit of a change the person decided in this decision, which
    /// builds approves when it records the decision.
    Decided(Id),
    /// The person's approval stays: the range's last commit draws every
    /// board as they approved it.
    Kept,
    /// The amend cleared the person's approval.
    Cleared,
    /// The change waits on the person's first review.
    Waits,
}

/// What a record or amend printed: the change, where it stands with the
/// person's review, and the items waiting on it.
fn recorded_words(verb: &str, recorded: &design::Recorded, title: &str, review: Review) -> String {
    let id = recorded.design;
    let mut out = format!("{verb} design change {id} \"{title}\"\n");
    let waits = format!(
        "design change {id} waits on the person's review (the Waiting on you tab); builds land --design refuses it until `builds design approve --by-person`\n"
    );
    match review {
        Review::Fix => out.push_str(&format!(
            "it is a fix that changes no design, so it needs no review; builds land --design {} can land it\n",
            id.0
        )),
        Review::Decided(decision) => out.push_str(&format!(
            "it is a board edit of a change the person decided in decision {decision}, so it needs no review; builds land --design {} can land it\n",
            id.0
        )),
        Review::Kept => out.push_str(&format!(
            "the person's approval stays: every board is byte for byte as they approved it; builds land --design {} can land it\n",
            id.0
        )),
        Review::Cleared => {
            out.push_str(
                "the person's approval is cleared: they approved what was drawn before this\n",
            );
            out.push_str(&waits);
        }
        Review::Waits => out.push_str(&waits),
    }
    if !recorded.new_items.is_empty() {
        let ids: Vec<String> = recorded.new_items.iter().map(|id| id.to_string()).collect();
        out.push_str(&format!("new items {} wait on it\n", ids.join(", ")));
    }
    out
}

fn record(tool: &Tool, args: &[String], cwd: &Path, session: Option<&Messaging>) -> Result<String> {
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
                tip: &scope.tip,
                same_as: None,
            },
            &request,
            &scope.changes,
            &affected,
            &stamp,
        )?;
        design_session::keep(db, recorded.design, session)?;
        let title = store::read(db)?
            .design(recorded.design)
            .expect("just recorded")
            .title
            .clone();
        Ok((recorded, title))
    })?;
    let review = match request.settlement {
        Settlement::Fix(_) => Review::Fix,
        Settlement::Decision(decision) => Review::Decided(decision),
        Settlement::Review => Review::Waits,
    };
    Ok(recorded_words("recorded", &recorded, &title, review))
}

fn amend(tool: &Tool, args: &[String], cwd: &Path, session: Option<&Messaging>) -> Result<String> {
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
    let same_as = drawn_the_same(&scope, id)?;
    let (recorded, title, review) =
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
                        tip: &scope.tip,
                        same_as: same_as.as_deref(),
                    },
                    &request,
                    &scope.changes,
                    &affected,
                    &stamp,
                )?;
                design_session::keep(db, id, session)?;
                let lists = store::read(db)?;
                let design = lists.design(id).expect("open");
                let review = match (&design.settlement, design.approved.is_some()) {
                    (Settlement::Fix(_), _) => Review::Fix,
                    (Settlement::Decision(decision), _) => Review::Decided(*decision),
                    (Settlement::Review, true) => Review::Kept,
                    (Settlement::Review, false) if was_approved => Review::Cleared,
                    (Settlement::Review, false) => Review::Waits,
                };
                Ok((recorded, design.title.clone(), review))
            })?;
    Ok(recorded_words("amended", &recorded, &title, review))
}

/// The last commit of open change <id> of the scope's repo, when the range's
/// last commit draws every board and component place of the change, and
/// every path the range changes that a drawing shows (`Changes::drawn`),
/// byte for byte as that commit does; None when the range's last commit
/// draws one of these paths differently, or the change keeps no last commit. An amend keeps the person's approval
/// only then (`design::amend`).
fn drawn_the_same(scope: &Scope, id: Id) -> Result<Option<String>> {
    let Some(design) = scope.lists.design(id) else {
        return Ok(None);
    };
    let Some(tip) = &design.tip else {
        return Ok(None);
    };
    if design.repo != scope.checkout.repo() {
        return Ok(None);
    }
    let paths: BTreeSet<&String> = design
        .places
        .keys()
        .filter(|place| PlaceKind::of(place) != PlaceKind::Spec)
        .chain(scope.changes.drawn())
        .collect();
    let top = &scope.checkout.top;
    let before = design_paths::drawing_at(top, tip, paths.iter().copied())?;
    let after = design_paths::drawing_at(top, &scope.tip, paths.iter().copied())?;
    Ok((before == after).then(|| tip.clone()))
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
        "design change {id} approved; builds land --design {} can land it\n",
        id.0
    ))
}

/// `check N[,N...] DIR REV...`: the changes together resolve every place the
/// commits change, and the person has approved each. `check` reads the store
/// and never writes to it.
fn check(tool: &Tool, args: &[String]) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [ids, dir, shas @ ..] = args.positional() else {
        fail!("{USAGE}")
    };
    if shas.is_empty() {
        fail!("{USAGE}");
    }
    let ids = Id::parse_list(ids)?;
    let scope = named_scope(tool, &args, dir, shas)?;
    let top = &scope.checkout.top;
    let drawn: BTreeSet<&String> = scope.changes.drawn().collect();
    let designs = design::open_named(&scope.lists, &ids, &scope.checkout.repo())?;
    let drawings = design::Drawings::read(top, &scope.tip, designs, &drawn)?;
    design::check_changes(&drawings.changes, &scope.changes, &drawings.now)?;
    let named = design::named(&ids);
    Ok(match ids.as_slice() {
        [_] => format!("{named} resolves every place the commits change and is approved\n"),
        _ => format!(
            "{named} together resolve every place the commits change and each is approved\n"
        ),
    })
}

/// `landed N DIR SHA...`: SHAs are on origin/main, as builds land calls it after
/// the push; they are stored as full hashes. Who landed is `--by`, else the
/// claim of DIR, the worktree builds land runs in.
fn landed(tool: &Tool, args: &[String]) -> Result<String> {
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
    let stamp = Stamp::resolve(tool.now, &args, &checkout.top)?;
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

/// `drop N --why WHY`: closes design change N, which will not land, and
/// records the reason WHY.
fn drop(tool: &Tool, args: &[String], cwd: &Path) -> Result<String> {
    let args = Args::parse(args, &["why"], &[])?;
    let [id] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let id = Id::parse(id)?;
    let why = args.why()?;
    let store = Store::resolve(&tool.places, &args, cwd)?;
    let stamp = Stamp::resolve(tool.now, &args, cwd)?;
    store.write(&stamp, &format!("design drop {id}"), |db| {
        design::drop(db, id, why, &stamp)
    })?;
    Ok(format!("closed design change {id}; it will not land\n"))
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

/// `render DIR BOARD [--theme light|dark] [--now | --at REV]`: the board as one
/// page of HTML, drawn with the design system it is drawn from. BOARD is the
/// board's name (`mac.welcome.open-failed`) or its path
/// (`design/mocks/<name>.dc.html`). It reads the board's file in worktree DIR
/// as it is; `--now` reads it as it stands on origin/main, and `--at REV` as
/// it is at commit or branch REV (a change's branch, to look at its proposed
/// board with nothing checked out).
fn render(tool: &Tool, args: &[String]) -> Result<String> {
    let args = Args::parse(args, &["theme", "at"], &["now"])?;
    let [dir, board] = args.exactly(2, USAGE)? else {
        unreachable!("exactly two")
    };
    let theme = match args.value("theme")? {
        None | Some("dark") => crate::lists::Theme::Dark,
        Some("light") => crate::lists::Theme::Light,
        Some(other) => fail!("--theme is light or dark, not {other:?}"),
    };
    let dir = Path::new(dir);
    let source = match (args.switch("now"), args.value("at")?) {
        (true, Some(_)) => fail!("--now and --at both say which drawing: give one"),
        (true, None) => boards::Source::Commit {
            repo: dir,
            rev: crate::design_render::NOW,
        },
        (false, Some(rev)) => boards::Source::Commit { repo: dir, rev },
        (false, None) => boards::Source::Dir(dir),
    };
    crate::design_render::render_board(source, board, theme, &tool.places.design_system_cache())
}

/// `tokens-css DIR`: writes DIR's design/system/tokens.css from its
/// tokens.json, as builds draws boards with it.
fn tokens_css(args: &[String]) -> Result<String> {
    let args = Args::parse(args, &[], &[])?;
    let [dir] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let dir = Path::new(dir);
    let css = boards::Source::Dir(dir).tokens_css()?;
    let to = dir.join(boards::TOKENS_CSS);
    std::fs::write(&to, css).context(|| format!("writing {}", to.display()))?;
    Ok(format!("wrote {}\n", to.display()))
}

/// `publish-files DIR OUT [--at REV]`: writes into folder OUT the files of
/// every artifact the designs of DIR's repo publish to, each canvas and the
/// design system, all from the one commit that origin/main or REV names, and
/// prints each artifact's folder and calls
/// (`design_publish::write_all`).
fn publish_files(tool: &Tool, args: &[String]) -> Result<String> {
    let args = Args::parse(args, &["at"], &[])?;
    let [dir, out] = args.exactly(2, USAGE)? else {
        unreachable!("exactly two")
    };
    let rev = args.value("at")?.unwrap_or(crate::design_render::NOW);
    let publish = crate::design_publish::write_all(
        Path::new(dir),
        rev,
        Path::new(out),
        &tool.places.design_system_cache(),
    )?;
    Ok(crate::design_publish::report(&publish))
}

/// `publish-last FOLDER --listing FILE [--index FILE --by WHO]`: writes the
/// last calls of the artifact whose folder `publish-files` wrote, from the
/// artifact's listing (FILE, or `-` for standard input) and, for the design
/// system, its live index (`design_publish::write_last`).
fn publish_last(args: &[String]) -> Result<String> {
    let args = Args::parse(args, &["listing", "index", "by"], &[])?;
    let [folder] = args.exactly(1, USAGE)? else {
        unreachable!("exactly one")
    };
    let Some(listing) = args.value("listing")? else {
        fail!("give the artifact's listing with --listing FILE, or - for standard input")
    };
    let listing = crate::lists::args::read_input(listing)?;
    let index = match (args.value("index")?, args.value("by")?) {
        (None, None) => None,
        (Some(index), Some(by)) => Some((
            std::fs::read_to_string(index).context(|| format!("reading {index}"))?,
            by.to_string(),
        )),
        _ => fail!("--index and --by go together: the index to send, and who publishes it"),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs() as i64;
    let root = Path::new(folder);
    let calls = crate::design_publish::write_last(
        root,
        &listing,
        index
            .as_ref()
            .map(|(text, by)| (text.as_str(), by.as_str())),
        &crate::text::utc_second(now)?,
    )?;
    Ok(crate::design_publish::last_report(root, &calls))
}
