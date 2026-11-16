//! Design changes: a product design change (a board or a design-system
//! component) recorded when its boards are drawn, with every design place it
//! changes resolved to the app work that follows or to how it was checked
//! that none does, and waiting on the person's review until it can land.
//! These are the operations on the state a write holds under the lock, and
//! the rule that decides which open change covers commits (`covers`,
//! `design_commits`), which reads git through `design_paths`;
//! `design_command` is the commands around them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::db::Sql;
use crate::fail;
use crate::fail::Result;
use crate::lists::derive::{self, ItemState};
use crate::lists::design_paths::{Changes, Drawing, Globs, PlaceKind, changes_of, drawing_at};
use crate::lists::design_session::{self, SessionState};
use crate::lists::item::{ItemDraft, RecordWait, add_item};
use crate::lists::record;
use crate::lists::store;
use crate::lists::{Closed, How, Id, Kind, Lists, Located, Record, Stamp, Standing, Wait};
use crate::text::local_minute;

/// How a design place was resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    /// The item that does the app work for it.
    Item(Id),
    /// How it was checked that no app work is needed.
    Matches(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignLanding {
    pub commits: Vec<String>,
    pub at: i64,
}

/// A board place's link on the published canvas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Board {
    /// The board's link on the canvas.
    pub link: String,
}

/// The theme a board is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn word(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

/// A board on a review page: the board file (a place of the repo's design
/// places, `design/mocks/<name>.dc.html`), what the caption under it says,
/// and the theme it is drawn in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewBoard {
    pub board: String,
    pub caption: String,
    pub theme: Theme,
}

/// One heading of a review page, the words under it, and the boards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSection {
    pub heading: String,
    #[serde(default)]
    pub words: String,
    #[serde(default)]
    pub boards: Vec<ReviewBoard>,
}

/// A change to the design files, recorded when its boards are drawn. It
/// waits on the person's review (`approved`) before it can land.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignChange {
    pub id: Id,
    pub title: String,
    pub details: String,
    pub repo: String,
    /// The worktree (absolute path) of the repo the boards are drawn from.
    pub worktree: String,
    /// The last commit of the change's range: the review page draws each
    /// board from the files at it, so the worktree can have any branch
    /// checked out. None for a change recorded before the tip was kept, until
    /// it is amended.
    pub tip: Option<String>,
    /// Each design place the change touches, or a component a shared source
    /// changes the drawing of, and how it is resolved.
    pub places: BTreeMap<String, Resolution>,
    /// Each board place's canvas link, by the board's path; none in a
    /// project that publishes no canvas.
    pub boards: BTreeMap<String, Board>,
    /// The board files the change deletes: they need no answer or link, and
    /// the review page names them without a drawing.
    pub deleted: BTreeSet<String>,
    /// How the review page reads: its headings, words and boards, in order.
    pub review: Vec<ReviewSection>,
    /// Why shared design files changed and no drawing did.
    pub no_app_place: Option<String>,
    /// What lets the change land: the person's review (`design approve`),
    /// or the recording of a fix. An amend clears it unless every board
    /// stays as the person approved it (`amend`).
    pub approved: Option<Stamp>,
    /// What a change that changes no design fixes (a design-system measure,
    /// a drawing mistake): it is approved when recorded, with no review.
    pub fix: Option<String>,
    /// The session that recorded it, and whether the approval reached it.
    pub session: SessionState,
    pub opened: Stamp,
    pub landed: Vec<DesignLanding>,
}

impl DesignChange {
    /// Does the change wait on the person? The change waits on their review
    /// until they approve it, and after that only on its landing.
    pub fn waits_on_person(&self) -> bool {
        self.approved.is_none()
    }
}

impl Lists {
    pub fn design(&self, id: Id) -> Option<&DesignChange> {
        self.designs.iter().find(|d| d.id == id)
    }
}

/// How one changed place is resolved in a request.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceAnswer {
    /// An open item that does the app work for it.
    Item(Id),
    /// A new item for the app work; it waits on this design change.
    New(ItemDraft),
    /// How it was checked that the app already matches.
    Matches(String),
}

/// A design change as `design record` and `design amend` take it: `title`
/// and `details` are required to record and optional to amend.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignRequest {
    pub title: Option<String>,
    pub details: Option<String>,
    /// A worktree of the repo the commits are in.
    pub worktree: String,
    /// The commits, e.g. `origin/main..HEAD`.
    pub range: String,
    /// For every design place the range changes, and for each component a
    /// changed shared source changes the drawing of.
    #[serde(default)]
    pub places: BTreeMap<String, PlaceAnswer>,
    /// Each board place's canvas link, which a project that publishes a
    /// canvas gives for every board and any other project leaves out.
    #[serde(default)]
    pub boards: BTreeMap<String, Board>,
    /// How the review page reads: headings, the words under each, and its
    /// boards, which the dashboard draws from the range's last commit. Required
    /// to record; an amend that gives it replaces the page, one that doesn't
    /// keeps it. Every board place of the change must be on it.
    pub review: Option<Vec<ReviewSection>>,
    /// Why shared design files changed and no drawing did.
    pub no_app_place: Option<String>,
    /// What the change fixes when it changes no design: boards made to follow
    /// the design system's measures, or a drawing mistake corrected. A fix
    /// needs no review page and is approved when recorded; an amend without
    /// it makes the change wait on the person's review again.
    pub fix: Option<String>,
}

/// What a record or amend made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub design: Id,
    pub new_items: Vec<Id>,
}

fn require_text(what: &str, text: Option<&String>) -> Result<String> {
    match text {
        Some(text) if !text.trim().is_empty() => Ok(text.clone()),
        Some(_) => fail!("{what} is empty"),
        None => fail!("a design change needs its {what}"),
    }
}

fn open_item(db: &Connection, id: Id) -> Result<()> {
    let lists = store::read(db)?;
    match lists.locate(id) {
        None => fail!("place resolved by item {id}, which no record has"),
        Some(Located { kind, .. }) if kind != Kind::Item => {
            fail!("place resolved by item {id}, but it is a {}", kind.name())
        }
        Some(Located {
            standing: Standing::Closed(closed),
            ..
        }) => {
            fail!(
                "place resolved by item {id}, which is closed ({})",
                closed.how.label()
            )
        }
        Some(_) => Ok(()),
    }
}

/// Resolves <changes> in design change <id> by <request>: each answer is
/// applied (a new item waits on the change), then every changed place must
/// be resolved and the shared files and sources answered for. <affected> are
/// the request's places that name existing components rather than something
/// the range changes. Refuses with every unresolved place named.
fn resolve(
    db: &Connection,
    id: Id,
    request: &DesignRequest,
    drawn: &Drawn,
    changes: &Changes,
    affected: &BTreeSet<String>,
    stamp: &Stamp,
) -> Result<Vec<Id>> {
    if !changes.is_design_change() {
        fail!("the range {} changes no design files", request.range);
    }
    let changed = changes.resolvable();
    if let Some(path) = request.places.keys().find(|p| changes.deleted.contains(*p)) {
        fail!("`places` names {path}, which the range deletes; a deleted board needs no answer");
    }
    let stray: Vec<&str> = request
        .places
        .keys()
        .filter(|path| !changed.contains(*path) && !affected.contains(*path))
        .map(String::as_str)
        .collect();
    if !stray.is_empty() {
        fail!(
            "`places` names {}, which the range does not change{}",
            stray.join(", "),
            if changes.sources.is_empty() {
                ""
            } else {
                " and which is no component folder of the design system; a changed shared source takes the components it changes the drawing of"
            }
        );
    }
    if !affected.is_empty() && changes.sources.is_empty() {
        fail!(
            "`places` names {}, which the range does not change; components that are not in it are listed only when it changes shared design-system sources",
            join(affected)
        );
    }
    let mut resolutions = BTreeMap::new();
    let mut new_items = Vec::new();
    for (path, answer) in &request.places {
        let resolution = match answer {
            PlaceAnswer::Item(item) => {
                open_item(db, *item)?;
                Resolution::Item(*item)
            }
            PlaceAnswer::New(draft) => {
                let mut draft = draft.clone();
                if !draft.waits.contains(&Wait::Design(id)) {
                    draft.waits.push(Wait::Design(id));
                }
                let item = add_item(db, &draft, Some(id), stamp)?;
                new_items.push(item);
                Resolution::Item(item)
            }
            PlaceAnswer::Matches(how) if how.trim().is_empty() => {
                fail!("place {path} is `matches` with no how it was checked")
            }
            PlaceAnswer::Matches(how) => Resolution::Matches(how.clone()),
        };
        resolutions.insert(path.clone(), resolution);
    }
    let mut held = store::read(db)?;
    if !held.canvas && !request.boards.is_empty() {
        fail!(
            "project {} publishes no canvas, so its design changes take no canvas links; leave out `boards`",
            held.project
        );
    }
    for (path, board) in &request.boards {
        if PlaceKind::of(path) != PlaceKind::Board {
            fail!("`boards` names {path}, which is no board file");
        }
        if board.link.trim().is_empty() {
            fail!("board {path} has an empty canvas link");
        }
    }
    if let Some(review) = &request.review {
        check_review(review)?;
    }
    let canvas = held.canvas;
    let Some(at) = held.designs.iter().position(|d| d.id == id) else {
        fail!("{id} is not an open design change")
    };
    let design = &mut held.designs[at];
    design.worktree = drawn.worktree.to_string();
    design.tip = Some(drawn.tip.to_string());
    // The change holds what the range changes: a stored place it no longer
    // touches (a board renamed before landing) leaves with its board and its
    // place on the review page.
    let dropped: BTreeSet<String> = design
        .places
        .keys()
        .filter(|p| !changed.contains(*p) && !affected.contains(*p))
        .cloned()
        .collect();
    design.places.retain(|path, _| !dropped.contains(path));
    design.boards.retain(|path, _| !dropped.contains(path));
    for section in &mut design.review {
        section.boards.retain(|b| !dropped.contains(&b.board));
    }
    design.deleted = changes.deleted.clone();
    design.places.extend(resolutions);
    design.boards.extend(request.boards.clone());
    if let Some(review) = &request.review {
        design.review = review.clone();
    }
    let unresolved: Vec<&str> = changed
        .iter()
        .filter(|p| !design.places.contains_key(**p))
        .map(|p| p.as_str())
        .collect();
    if !unresolved.is_empty() {
        fail!(
            "design places left unresolved: {}; give each `item` (an open item), `new` (an item draft) or `matches` (how it was checked)",
            unresolved.join(", ")
        );
    }
    let unlinked: Vec<&str> = design
        .places
        .keys()
        .filter(|p| PlaceKind::of(p) == PlaceKind::Board && !design.boards.contains_key(*p))
        .map(String::as_str)
        .collect();
    if canvas && !unlinked.is_empty() {
        fail!(
            "boards without a canvas link: {}; give each under `boards` as {{\"link\": CANVAS_URL}}",
            unlinked.join(", ")
        );
    }
    if let Some(path) = design
        .boards
        .keys()
        .find(|p| !design.places.contains_key(*p))
    {
        fail!("`boards` names {path}, which is no place of this design change");
    }
    let on_page: BTreeSet<&str> = design
        .review
        .iter()
        .flat_map(|s| &s.boards)
        .map(|b| b.board.as_str())
        .collect();
    let missing: Vec<&str> = design
        .places
        .keys()
        .filter(|p| PlaceKind::of(p) == PlaceKind::Board && !on_page.contains(p.as_str()))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() && request.fix.is_none() {
        fail!(
            "boards of the change missing from the review page: {}; the person reviews every board of a change on its page, so give each under `review`",
            missing.join(", ")
        );
    }
    if let Some(why) = &request.no_app_place {
        if why.trim().is_empty() {
            fail!("no_app_place is empty; say why shared design files changed and no drawing did");
        }
        // `no_app_place` is valid when design-system sources change, whatever
        // boards change with them, or when shared design files change and no
        // board does. It is invalid when the change lists or changes a
        // component, because that component's drawing changes.
        let drawing = lists_a_component(design);
        let answers = !changes.sources.is_empty()
            || (!changes.shared.is_empty() && changes.places.is_empty());
        if !answers || drawing {
            fail!(
                "no_app_place is only for a range that changes design-system sources, or shared design files and no board, and record rejects it when the change lists or changes a component whose drawing changes"
            );
        }
        design.no_app_place = Some(why.clone());
    }
    needs_answer(&[&*design], changes, "the range")?;
    save(db, design)?;
    Ok(new_items)
}

/// A review page's own rules: every section has a heading, and every board on
/// it is a board file. (That a board is one of the repo's board places and
/// exists in the worktree is the command's, which reads git and the files.)
fn check_review(review: &[ReviewSection]) -> Result<()> {
    if review.is_empty() {
        fail!("`review` has no section; a review page has at least one heading");
    }
    for section in review {
        if section.heading.trim().is_empty() {
            fail!("a section of `review` has an empty heading");
        }
        for board in &section.boards {
            if PlaceKind::of(&board.board) != PlaceKind::Board {
                fail!("`review` names {}, which is no board file", board.board);
            }
            if board.caption.trim().is_empty() {
                fail!(
                    "the board {} on the review page has an empty caption",
                    board.board
                );
            }
        }
    }
    Ok(())
}

/// Shared design files and sources change no place by themselves: one of
/// <designs> must list the components whose drawing changes (a source) or
/// have a resolved place, or say with `no_app_place` that no drawing changes.
fn needs_answer(designs: &[&DesignChange], changes: &Changes, what: &str) -> Result<()> {
    if designs.iter().any(|d| d.no_app_place.is_some()) {
        return Ok(());
    }
    if !changes.sources.is_empty() {
        if designs.iter().any(|d| lists_a_component(d)) {
            return Ok(());
        }
        fail!(
            "{what} changes shared design-system sources ({}); list under `places` each component whose drawing changes (an item or `matches`), or say with `no_app_place` that no drawing changes",
            join(&changes.sources)
        );
    }
    if !changes.shared.is_empty() && changes.places.is_empty() && changes.spec.is_empty() {
        fail!(
            "{what} changes only shared design files ({}); say why no board or component changed with `no_app_place`",
            join(&changes.shared)
        );
    }
    Ok(())
}

/// Does <design> list a component folder among its places?
fn lists_a_component(design: &DesignChange) -> bool {
    design
        .places
        .keys()
        .any(|p| PlaceKind::of(p) == PlaceKind::Component)
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a design change starts as: no places, boards or review yet.
pub struct NewDesign {
    pub title: String,
    pub details: String,
    pub repo: String,
    pub worktree: String,
    pub no_app_place: Option<String>,
}

/// Opens a design change as <new>, made by <stamp>; returns its id.
pub fn open_change(db: &Connection, new: NewDesign, stamp: &Stamp) -> Result<Id> {
    let id = record::open(db, Kind::Design, stamp)?;
    db.execute(
        "INSERT INTO design (id, title, details, repo, worktree, no_app_place) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, new.title, new.details, new.repo, new.worktree, new.no_app_place],
    )
    .sql()?;
    Ok(id)
}

/// Writes what the change holds of its own: its fields, and its places,
/// boards and review page as a whole, which replace the ones it had. The
/// approval is the person's and is not touched.
pub fn save(db: &Connection, design: &DesignChange) -> Result<()> {
    db.execute(
        "UPDATE design SET title = ?2, details = ?3, worktree = ?4, no_app_place = ?5 WHERE id = ?1",
        params![design.id, design.title, design.details, design.worktree, design.no_app_place],
    )
    .sql()?;
    db.execute("DELETE FROM design_tip WHERE design_id = ?1", [design.id])
        .sql()?;
    if let Some(tip) = &design.tip {
        db.execute(
            "INSERT INTO design_tip (design_id, tip) VALUES (?1, ?2)",
            params![design.id, tip],
        )
        .sql()?;
    }
    // The boards leave with their places, the review boards with their sections.
    db.execute("DELETE FROM design_place WHERE design_id = ?1", [design.id])
        .sql()?;
    db.execute(
        "DELETE FROM review_section WHERE design_id = ?1",
        [design.id],
    )
    .sql()?;
    for (path, resolution) in &design.places {
        let (item, matches) = match resolution {
            Resolution::Item(item) => (Some(*item), None),
            Resolution::Matches(how) => (None, Some(how.as_str())),
        };
        db.execute(
            "INSERT INTO design_place (design_id, path, item_id, matches) VALUES (?1, ?2, ?3, ?4)",
            params![design.id, path, item, matches],
        )
        .sql()?;
    }
    db.execute(
        "DELETE FROM design_deleted WHERE design_id = ?1",
        [design.id],
    )
    .sql()?;
    for path in &design.deleted {
        db.execute(
            "INSERT INTO design_deleted (design_id, path) VALUES (?1, ?2)",
            params![design.id, path],
        )
        .sql()?;
    }
    for (path, board) in &design.boards {
        db.execute(
            "INSERT INTO design_board (design_id, path, link) VALUES (?1, ?2, ?3)",
            params![design.id, path, board.link],
        )
        .sql()?;
    }
    for (position, section) in design.review.iter().enumerate() {
        db.execute(
            "INSERT INTO review_section (design_id, position, heading, words) VALUES (?1, ?2, ?3, ?4)",
            params![design.id, position as i64, section.heading, section.words],
        )
        .sql()?;
        for (at, board) in section.boards.iter().enumerate() {
            db.execute(
                "INSERT INTO review_board (design_id, section, position, board, caption, theme) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![design.id, position as i64, at as i64, board.board, board.caption, board.theme.word()],
            )
            .sql()?;
        }
    }
    Ok(())
}

/// The checkout a design change is drawn from: its repo, the worktree
/// (absolute path) of it, and the commit the boards are drawn at.
#[derive(Clone, Copy)]
pub struct Drawn<'a> {
    pub repo: &'a str,
    pub worktree: &'a str,
    /// The last commit of the range the boards are drawn at.
    pub tip: &'a str,
    /// For an amend, the change's last commit before it, when `tip` draws
    /// every place of the change byte for byte as that commit does; None
    /// when a place is drawn differently, and for a record.
    pub same_as: Option<&'a str>,
}

/// Records a new design change of <repo> for <changes>.
pub fn record(
    db: &Connection,
    drawn: &Drawn,
    request: &DesignRequest,
    changes: &Changes,
    affected: &BTreeSet<String>,
    stamp: &Stamp,
) -> Result<Recorded> {
    let Drawn { repo, .. } = *drawn;
    let title = require_text("title", request.title.as_ref())?;
    if let Some(fault) = crate::lists::check::title_fault(&title) {
        fail!("{fault}");
    }
    let details = require_text("details", request.details.as_ref())?;
    if request.review.is_none() && request.fix.is_none() {
        fail!(
            "a design change needs its `review`: the headings, words and boards of the page the person reviews it on (a `fix` that changes no design needs none)"
        );
    }
    let design = open_change(
        db,
        NewDesign {
            title,
            details,
            repo: repo.to_string(),
            worktree: drawn.worktree.to_string(),
            no_app_place: None,
        },
        stamp,
    )?;
    let new_items = resolve(db, design, request, drawn, changes, affected, stamp)?;
    keep_fix(db, design, request, stamp)?;
    Ok(Recorded { design, new_items })
}

/// Writes <request>'s fix of change <id>, which approves it as recorded by
/// <stamp>, or leaves the change with no fix.
fn keep_fix(db: &Connection, id: Id, request: &DesignRequest, stamp: &Stamp) -> Result<()> {
    db.execute("DELETE FROM design_fix WHERE design_id = ?1", [id])
        .sql()?;
    let Some(fixes) = &request.fix else {
        return Ok(());
    };
    if fixes.trim().is_empty() {
        fail!(
            "fix is empty; say what the change fixes (the design system's measures, a drawing mistake)"
        );
    }
    db.execute(
        "INSERT INTO design_fix (design_id, fixes) VALUES (?1, ?2)",
        params![id, fixes],
    )
    .sql()?;
    approve(db, id, stamp)
}

/// The open design change <id>.
fn open_design(lists: &Lists, id: Id) -> Result<&DesignChange> {
    Ok(match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { kind, .. }) if kind != Kind::Design => {
            fail!("{id} is a {}, not a design change", kind.name())
        }
        Some(Located {
            standing: Standing::Closed(closed),
            ..
        }) => fail!("{}", how_it_closed(id, closed)),
        Some(_) => lists.design(id).expect("located open"),
    })
}

/// How design change <id> closed, as <closed> says: it landed, or someone
/// dropped it, the two ways `closing_how_fits_kind` lets a design change
/// close.
fn how_it_closed(id: Id, closed: &Closed) -> String {
    match &closed.how {
        How::Landed(_) => format!("design change {id} has already landed"),
        How::Dropped(why) => format!("{} dropped design change {id}: {why}", closed.closed.by),
        other => unreachable!("closing_how_fits_kind holds, but {id} closed as {other:?}"),
    }
}

/// The open design change <id>, of <repo>.
fn open_design_of<'a>(lists: &'a Lists, id: Id, repo: &str) -> Result<&'a DesignChange> {
    let design = open_design(lists, id)?;
    if design.repo != repo {
        fail!("design change {id} is of repo {}, not {repo}", design.repo);
    }
    Ok(design)
}

/// Holds open design change <id> to the places <changes> holds: the answers
/// of <request> are added, and a stored place the range no longer changes
/// leaves with its board and its place on the review page. The person's
/// approval stays only when what they approved stays: the same places,
/// deleted boards and review page, every place drawn byte for byte as the
/// last commit they approved draws it (`Drawn::same_as`), and no fix turned
/// into a change that needs their review. Otherwise the amend clears it.
pub fn amend(
    db: &Connection,
    id: Id,
    drawn: &Drawn,
    request: &DesignRequest,
    changes: &Changes,
    affected: &BTreeSet<String>,
    stamp: &Stamp,
) -> Result<Recorded> {
    let Drawn { repo, same_as, .. } = *drawn;
    let before = open_design_of(&store::read(db)?, id, repo)?.clone();
    if let Some(title) = &request.title {
        if title.trim().is_empty() {
            fail!("title is empty");
        }
        if let Some(fault) = crate::lists::check::title_fault(title) {
            fail!("{fault}");
        }
        db.execute(
            "UPDATE design SET title = ?2 WHERE id = ?1",
            params![id, title],
        )
        .sql()?;
    }
    if let Some(details) = &request.details {
        let details = require_text("details", Some(details))?;
        db.execute(
            "UPDATE design SET details = ?2 WHERE id = ?1",
            params![id, details],
        )
        .sql()?;
    }
    let new_items = resolve(db, id, request, drawn, changes, affected, stamp)?;
    let after = store::read(db)?;
    let after = after.design(id).expect("resolve keeps the change open");
    let approval_holds = same_as.is_some()
        && same_as == before.tip.as_deref()
        && before.places.keys().eq(after.places.keys())
        && before.deleted == after.deleted
        && before.review == after.review
        && !(before.fix.is_some() && request.fix.is_none());
    if !approval_holds {
        db.execute("DELETE FROM design_approval WHERE design_id = ?1", [id])
            .sql()?;
    }
    keep_fix(db, id, request, stamp)?;
    Ok(Recorded {
        design: id,
        new_items,
    })
}

/// How a message names the design changes <ids>: "design change #3" or
/// "design changes #3, #4".
pub fn named(ids: &[Id]) -> String {
    let list: Vec<String> = ids.iter().map(Id::to_string).collect();
    match ids {
        [_] => format!("design change {}", list[0]),
        _ => format!("design changes {}", list.join(", ")),
    }
}

/// The sentence that says how to add <what> to one of <ids>: to the one
/// change, or, when there are several, to the change <which> describes.
fn amend_hint(ids: &[Id], what: &str, which: &str) -> String {
    match ids {
        [id] => format!("`builds design amend {}` adds {what}.", id.0),
        _ => format!("`builds design amend N` adds {what}, where N is {which}."),
    }
}

/// Does <design> resolve or answer for something <changes> holds: a changed
/// place, a deleted board, or changed shared files and sources?
fn takes_part(design: &DesignChange, changes: &Changes) -> bool {
    let resolvable = changes.resolvable();
    design.places.keys().any(|p| resolvable.contains(p))
        || !design.deleted.is_disjoint(&changes.deleted)
        || (!changes.sources.is_empty() && lists_a_component(design))
        || (design.no_app_place.is_some()
            && !(changes.sources.is_empty() && changes.shared.is_empty()))
}

/// The open design changes <ids> of <repo>, in order, each named once.
pub fn open_named<'a>(lists: &'a Lists, ids: &[Id], repo: &str) -> Result<Vec<&'a DesignChange>> {
    let mut designs = Vec::new();
    for (at, id) in ids.iter().enumerate() {
        if ids[..at].contains(id) {
            fail!("design change {id} is named twice");
        }
        designs.push(open_design_of(lists, *id, repo)?);
    }
    Ok(designs)
}

/// A design change and how its last commit draws the paths of the commits
/// it is checked against (`Changes::drawn`, `drawn_by`).
pub struct DrawnChange<'a> {
    pub design: &'a DesignChange,
    pub drawing: Drawing,
}

/// Do <designs> together cover <changes>, the design files of commits whose
/// last commit draws them as <now>? Each place the commits change must be a
/// place of one of the changes. The person reviews each board as its
/// change's last commit draws it, so every changed path a drawing shows must
/// hold the same bytes there as in the commits: a board or component place in
/// a change that holds it, a shared design-system source in any of the
/// changes. The comparison reads files, not commit hashes, so commits rebased
/// onto a moved main stay covered while they draw the same boards. One of the
/// changes answers for the shared files and sources (`needs_answer`), and
/// each change resolves or answers for something the commits change: landing
/// closes every change it names, so one that takes no part would close with
/// commits that are not its own. Names each path left over. `builds design
/// check`, which land runs, and health's search for design commits that no
/// change covers both decide coverage here.
pub fn covers(designs: &[DrawnChange], changes: &Changes, now: &Drawing) -> Result<()> {
    let ids: Vec<Id> = designs.iter().map(|d| d.design.id).collect();
    let named = named(&ids);
    let holds = |d: &DrawnChange, place: &str| d.design.places.contains_key(place);
    let unresolved: Vec<&str> = changes
        .resolvable()
        .into_iter()
        .map(String::as_str)
        .filter(|place| !designs.iter().any(|d| holds(d, place)))
        .collect();
    if !unresolved.is_empty() {
        let verb = if ids.len() == 1 { "does" } else { "do" };
        fail!(
            "{named} {verb} not resolve {}. {}",
            unresolved.join(", "),
            amend_hint(&ids, "each place", "the change that draws the place")
        );
    }
    let redrawn: Vec<&str> = changes
        .drawn()
        .map(String::as_str)
        .filter(|path| {
            !designs.iter().any(|d| {
                (changes.sources.contains(*path) || holds(d, path)) && d.drawing.same(now, path)
            })
        })
        .collect();
    if !redrawn.is_empty() {
        let pronoun = if redrawn.len() == 1 { "it" } else { "them" };
        fail!(
            "{named} drew {}, and the commits draw {pronoun} differently; the person approves each board as its change drew it. {}",
            redrawn.join(", "),
            amend_hint(
                &ids,
                "the boards and sources as the commits draw them",
                "the change that draws the path"
            )
        );
    }
    let plain: Vec<&DesignChange> = designs.iter().map(|d| d.design).collect();
    needs_answer(&plain, changes, "the commits").map_err(|error| match error {
        crate::fail::Fail::Message(message) => crate::fail::Fail::Message(format!(
            "{message} ({named}). {}",
            amend_hint(&ids, "the answer", "one of these changes")
        )),
        other => other,
    })?;
    if let Some(stray) = plain.iter().find(|design| !takes_part(design, changes)) {
        fail!(
            "design change {} resolves nothing the commits change, so landing them must not close it; leave it out",
            stray.id
        );
    }
    Ok(())
}

/// Do the open design changes <designs> cover <changes> (`covers`), and has
/// the person approved each? What `builds design check` decides, and builds
/// land with it.
pub fn check_changes(designs: &[DrawnChange], changes: &Changes, now: &Drawing) -> Result<()> {
    if !changes.is_design_change() {
        let ids: Vec<Id> = designs.iter().map(|d| d.design.id).collect();
        fail!(
            "the commits change no design files, so there is nothing to check for {}",
            named(&ids)
        );
    }
    covers(designs, changes, now)?;
    for DrawnChange { design, .. } in designs {
        let id = design.id;
        if design.approved.is_none() {
            fail!(
                "design change {id} waits on the person's review; once they have said to land it, `builds design approve {} --by-person` records that",
                id.0
            );
        }
    }
    Ok(())
}

/// How design change <design> draws <paths>: as its last commit has them,
/// or not at all for a change recorded before its last commit was kept.
pub fn drawn_by<'a>(
    top: &Path,
    design: &DesignChange,
    paths: impl IntoIterator<Item = &'a String>,
) -> Result<Drawing> {
    match &design.tip {
        Some(tip) => drawing_at(top, tip, paths),
        None => Ok(Drawing::default()),
    }
}

/// One commit past origin/main that changes design files, and the open
/// design change that covers it, if one does.
pub struct DesignCommit<'a> {
    pub sha: String,
    pub changes: Changes,
    pub covered_by: Option<&'a DesignChange>,
}

/// Each commit of <ahead> (the commits past origin/main, oldest first) in
/// the worktree at <top> of <repo> that changes design files, with the open
/// design change of the repo that covers it, approved or waiting on the
/// person's review. A change covers a commit as `covers` decides it
/// for `builds design check`, which land runs: it holds every place the
/// commit changes, and draws each changed path a drawing shows byte for byte
/// as the last of <ahead> does. The comparison reads files, so a change
/// still covers its commits after they are rebased onto a moved main.
/// Health and the dashboard's design card both read coverage here. Empty
/// when the lists name no design globs for the repo.
pub fn design_commits<'a>(
    lists: &'a Lists,
    repo: &str,
    top: &Path,
    ahead: &[String],
) -> Result<Vec<DesignCommit<'a>>> {
    let globs = Globs::of(lists, repo);
    if globs.places.is_empty() && globs.sources.is_empty() {
        return Ok(Vec::new());
    }
    let mut commits = Vec::new();
    for sha in ahead {
        let changes = changes_of(top, &globs, std::slice::from_ref(sha))?;
        if changes.is_design_change() {
            commits.push((sha, changes));
        }
    }
    if commits.is_empty() {
        return Ok(Vec::new());
    }
    let drawn: BTreeSet<&String> = commits
        .iter()
        .flat_map(|(_, changes)| changes.drawn())
        .collect();
    let last = ahead.last().expect("it holds the design commits");
    let now = drawing_at(top, last, drawn.iter().copied())?;
    let mut open = Vec::new();
    for design in open_designs(lists, repo) {
        open.push(DrawnChange {
            design,
            drawing: drawn_by(top, design, drawn.iter().copied())?,
        });
    }
    Ok(commits
        .into_iter()
        .map(|(sha, changes)| {
            let covered_by = open
                .iter()
                .find(|drawn| covers(std::slice::from_ref(*drawn), &changes, &now).is_ok())
                .map(|drawn| drawn.design);
            DesignCommit {
                sha: sha.clone(),
                changes,
                covered_by,
            }
        })
        .collect())
}

/// The open design changes of <repo>.
pub fn open_designs<'a>(lists: &'a Lists, repo: &str) -> impl Iterator<Item = &'a DesignChange> {
    lists.designs.iter().filter(move |design| {
        design.repo == repo
            && matches!(
                lists.locate(design.id),
                Some(Located {
                    standing: Standing::Open,
                    ..
                })
            )
    })
}

/// Records the person's approval of open design change <id>. The same
/// approval again is recorded once.
pub fn approve(db: &Connection, id: Id, stamp: &Stamp) -> Result<()> {
    open_design(&store::read(db)?, id)?;
    db.execute(
        "INSERT OR IGNORE INTO design_approval (design_id, at, by) VALUES (?1, ?2, ?3)",
        params![id, stamp.at, stamp.by],
    )
    .sql()?;
    Ok(())
}

/// What `landed` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Landed {
    /// The change closed; these items no longer wait on it and are ready.
    Closed { ready: Vec<(Id, String)> },
    /// The same commits were recorded before.
    Already,
}

/// Did design change <id> already land as exactly <commits>? A change that
/// landed as other commits is refused; one that is open is not landed yet.
pub fn already_landed(lists: &Lists, id: Id, repo: &str, commits: &[String]) -> Result<bool> {
    let Some(closed) = lists.closed_record(id) else {
        return Ok(false);
    };
    let Record::Design(design) = &closed.record else {
        fail!("{id} is not a design change that landed");
    };
    let How::Landed(was) = &closed.how else {
        fail!("{}", how_it_closed(id, closed));
    };
    if design.repo != repo {
        fail!("design change {id} is of repo {}, not {repo}", design.repo);
    }
    let (mut was, mut now) = (was.clone(), commits.to_vec());
    was.sort();
    now.sort();
    if was != now {
        fail!(
            "design change {id} already landed as {}, not as {}",
            was.join(" "),
            now.join(" ")
        );
    }
    Ok(true)
}

/// Records that design change <id> landed as <commits> (full hashes) and
/// closes it. The same commits again are recorded once; other commits for a
/// change that landed are refused.
pub fn landed(
    db: &Connection,
    id: Id,
    repo: &str,
    commits: &[String],
    stamp: &Stamp,
) -> Result<Landed> {
    if commits.is_empty() {
        fail!("design landed names no commits");
    }
    let before = store::read(db)?;
    if already_landed(&before, id, repo, commits)? {
        return Ok(Landed::Already);
    }
    open_design_of(&before, id, repo)?;
    record::close(db, id, How::Landed(commits.to_vec()), stamp)?;
    let lists = &store::read(db)?;
    let ready = lists
        .items
        .iter()
        .filter(|item| {
            item.waits.contains(&Wait::Design(id))
                && derive::item_state(lists, item) == ItemState::Ready
        })
        .map(|item| (item.id, item.title.clone()))
        .collect();
    Ok(Landed::Closed { ready })
}

/// Closes open design change <id> without landing it and records <why> as
/// the reason. drop refuses a change that is already closed, and one that an
/// open item waits on: closing that change would leave the item waiting on a
/// change that never lands.
pub fn drop(db: &Connection, id: Id, why: &str, stamp: &Stamp) -> Result<()> {
    let lists = &store::read(db)?;
    open_design(lists, id)?;
    lists.require_none_waits_on(RecordWait::Design(id), "drop")?;
    record::close(db, id, How::Dropped(why.to_string()), stamp)
}

fn resolution_words(lists: &Lists, resolution: &Resolution) -> String {
    match resolution {
        Resolution::Item(id) => match lists.locate(*id) {
            Some(Located {
                standing: Standing::Open,
                title,
                ..
            }) => {
                let state = lists
                    .item(*id)
                    .map(|i| derive::item_state(lists, i).label())
                    .unwrap_or("?");
                format!("item {id} [{state}] {title}")
            }
            Some(Located { title, .. }) => format!("item {id} [closed] {title}"),
            None => format!("item {id}"),
        },
        Resolution::Matches(how) => format!("matches: {how}"),
    }
}

fn describe(lists: &Lists, design: &DesignChange, closed: Option<&Closed>) -> String {
    let mut out = String::new();
    let state = match closed {
        Some(closed) => format!("{} {}", closed.how.label(), local_minute(closed.closed.at)),
        None => "open".to_string(),
    };
    let _ = writeln!(
        out,
        "{} {} [{state}] repo {}",
        design.id, design.title, design.repo
    );
    let _ = writeln!(
        out,
        "opened {} by {}",
        local_minute(design.opened.at),
        design.opened.by
    );
    if let Some(fixes) = &design.fix {
        let _ = writeln!(out, "a fix that changes no design: {fixes}");
    }
    match &design.approved {
        Some(stamp) => {
            let _ = writeln!(
                out,
                "approved {} (recorded by {})",
                local_minute(stamp.at),
                stamp.by
            );
        }
        None if closed.is_none() => {
            let _ = writeln!(out, "waiting on the person's review");
        }
        None => {}
    }
    let _ = writeln!(out, "\n{}\n", design.details.trim_end());
    for (path, resolution) in &design.places {
        let _ = writeln!(out, "  {path}  {}", resolution_words(lists, resolution));
    }
    for path in &design.deleted {
        let _ = writeln!(out, "  deleted {path}");
    }
    for (path, board) in &design.boards {
        let _ = writeln!(out, "  board {path}  {}", board.link);
    }
    let _ = writeln!(out, "  worktree {}", design.worktree);
    if let Some(tip) = &design.tip {
        let _ = writeln!(out, "  drawn at {tip}");
    }
    for section in &design.review {
        let _ = writeln!(out, "  review: {}", section.heading);
        for board in &section.boards {
            let _ = writeln!(
                out,
                "    {} ({}) {}",
                board.board,
                board.theme.word(),
                board.caption
            );
        }
    }
    if let Some(why) = &design.no_app_place {
        let _ = writeln!(out, "  no drawing changes: {why}");
    }
    for landing in &design.landed {
        let _ = writeln!(
            out,
            "  landed {} as {}",
            local_minute(landing.at),
            landing.commits.join(" ")
        );
    }
    if let Some(Closed {
        how: How::Dropped(why),
        ..
    }) = closed
    {
        let _ = writeln!(out, "  dropped: {why}");
    }
    out
}

/// `design show`: an open, landed or dropped design change.
pub fn show(lists: &Lists, id: Id) -> Result<String> {
    match lists.locate(id) {
        None => fail!("{id} is not a record"),
        Some(Located { kind, .. }) if kind != Kind::Design => {
            fail!("{id} is a {}, not a design change", kind.name())
        }
        Some(Located {
            standing: Standing::Open,
            ..
        }) => Ok(describe(lists, lists.design(id).expect("open"), None)),
        Some(Located {
            standing: Standing::Closed(closed),
            ..
        }) => {
            let Record::Design(design) = &closed.record else {
                unreachable!("located as a design change")
            };
            Ok(describe(lists, design, Some(closed)))
        }
    }
}

/// `design list`: the open design changes, one line each.
pub fn list(lists: &Lists) -> String {
    let mut out = String::new();
    for design in &lists.designs {
        let waiting = lists
            .items
            .iter()
            .filter(|i| i.waits.contains(&Wait::Design(design.id)))
            .count();
        let _ = writeln!(
            out,
            "{} {} ({}, repo {}, {} place{}, {waiting} item{} waiting)",
            design.id,
            design.title,
            if design.approved.is_some() {
                "approved"
            } else {
                "waiting on review"
            },
            design.repo,
            design.places.len(),
            if design.places.len() == 1 { "" } else { "s" },
            if waiting == 1 { "" } else { "s" },
        );
    }
    out
}

/// Every design change, open and closed, as the rows hold it. <opened> is
/// every record's stamp and <closings> every closing: a landed change's
/// landing is its closing.
pub fn read(
    db: &Connection,
    opened: &BTreeMap<Id, Stamp>,
    closings: &BTreeMap<Id, (How, Stamp)>,
) -> Result<Vec<DesignChange>> {
    let mut places: BTreeMap<Id, BTreeMap<String, Resolution>> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT design_id, path, item_id, matches FROM design_place")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Id>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .sql()?
    {
        let (design, path, item, matches) = row.sql()?;
        let resolution = match (item, matches) {
            (Some(item), _) => Resolution::Item(item),
            (None, how) => {
                Resolution::Matches(how.expect("design_place_is_resolved_one_way holds"))
            }
        };
        places.entry(design).or_default().insert(path, resolution);
    }
    let mut boards: BTreeMap<Id, BTreeMap<String, Board>> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT design_id, path, link FROM design_board")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .sql()?
    {
        let (design, path, link) = row.sql()?;
        boards
            .entry(design)
            .or_default()
            .insert(path, Board { link });
    }
    let mut deleted: BTreeMap<Id, BTreeSet<String>> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT design_id, path FROM design_deleted")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?
    {
        let (design, path) = row.sql()?;
        deleted.entry(design).or_default().insert(path);
    }
    let mut review: BTreeMap<Id, Vec<ReviewSection>> = BTreeMap::new();
    let mut statement = db
        .prepare(
            "SELECT design_id, heading, words FROM review_section ORDER BY design_id, position",
        )
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .sql()?
    {
        let (design, heading, words) = row.sql()?;
        review.entry(design).or_default().push(ReviewSection {
            heading,
            words,
            boards: Vec::new(),
        });
    }
    let mut statement = db
        .prepare("SELECT design_id, section, board, caption, theme FROM review_board ORDER BY design_id, section, position")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .sql()?
    {
        let (design, section, board, caption, theme) = row.sql()?;
        let theme = if theme == "light" {
            Theme::Light
        } else {
            Theme::Dark
        };
        review
            .get_mut(&design)
            .expect("review_board_is_on_a_section holds")[section as usize]
            .boards
            .push(ReviewBoard {
                board,
                caption,
                theme,
            });
    }
    let mut tips: BTreeMap<Id, String> = BTreeMap::new();
    let mut statement = db.prepare("SELECT design_id, tip FROM design_tip").sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?
    {
        let (design, tip) = row.sql()?;
        tips.insert(design, tip);
    }
    let mut approvals: BTreeMap<Id, Stamp> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT design_id, at, by FROM design_approval")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                Stamp {
                    at: row.get(1)?,
                    by: row.get(2)?,
                },
            ))
        })
        .sql()?
    {
        let (design, stamp) = row.sql()?;
        approvals.insert(design, stamp);
    }
    let mut fixes: BTreeMap<Id, String> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT design_id, fixes FROM design_fix")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, Id>(0)?, row.get::<_, String>(1)?))
        })
        .sql()?
    {
        let (design, fix) = row.sql()?;
        fixes.insert(design, fix);
    }
    let mut sessions = design_session::states(db)?;
    let mut statement = db
        .prepare("SELECT id, title, details, repo, worktree, no_app_place FROM design ORDER BY id")
        .sql()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .sql()?;
    let mut designs = Vec::new();
    for row in rows {
        let (id, title, details, repo, worktree, no_app_place) = row.sql()?;
        let landed = match closings.get(&id) {
            Some((How::Landed(commits), stamp)) => vec![DesignLanding {
                commits: commits.clone(),
                at: stamp.at,
            }],
            _ => Vec::new(),
        };
        designs.push(DesignChange {
            id,
            title,
            details,
            repo,
            worktree,
            tip: tips.remove(&id),
            places: places.remove(&id).unwrap_or_default(),
            boards: boards.remove(&id).unwrap_or_default(),
            deleted: deleted.remove(&id).unwrap_or_default(),
            review: review.remove(&id).unwrap_or_default(),
            no_app_place,
            approved: approvals.remove(&id),
            fix: fixes.remove(&id),
            session: sessions.remove(&id).unwrap_or(SessionState::Unrecorded),
            opened: opened[&id].clone(),
            landed,
        });
    }
    Ok(designs)
}

/// The tables design.rs owns.
#[cfg(test)]
const TABLES: [&str; 10] = [
    "design",
    "design_session",
    "design_tip",
    "design_approval",
    "design_fix",
    "design_place",
    "design_board",
    "design_deleted",
    "review_section",
    "review_board",
];

/// Writes a row of one of design.rs's tables as given, the constraints still
/// holding: for tests that need a row they refuse.
#[cfg(test)]
pub fn plant(
    db: &Connection,
    table: &str,
    columns: &[(&str, rusqlite::types::Value)],
) -> rusqlite::Result<usize> {
    crate::db::plant_row(db, &TABLES, table, columns)
}

#[cfg(test)]
#[path = "design_tests.rs"]
mod tests;
