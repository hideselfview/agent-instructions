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
use crate::lists::design_variants::{self, Variants};
use crate::lists::item::{ItemDraft, RecordWait, add_item};
use crate::lists::record;
use crate::lists::store;
use crate::lists::{
    Closed, Decision, How, Id, Kind, Lists, Located, Record, Stamp, Standing, Wait,
};
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

    /// The theme <word> names, the inverse of `word`.
    pub fn of_word(word: &str) -> Result<Theme> {
        match word {
            "light" => Ok(Theme::Light),
            "dark" => Ok(Theme::Dark),
            other => fail!("{other:?} is no theme: a theme is light or dark"),
        }
    }
}

/// A board on a review page: the board file (a place of the repo's design
/// places, `design/mocks/<name>.dc.html`), what the caption under it says,
/// the theme it is drawn in, and the elements the designer circles on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewBoard {
    pub board: String,
    pub caption: String,
    pub theme: Theme,
    /// What changed, as the designer says: the page circles these and
    /// nothing else. On a board the change adds, the circles mark what it
    /// adds, and the page draws them on the Proposed side only.
    #[serde(default)]
    pub circles: Vec<Circle>,
}

/// A circle the designer draws on a review board: the area it surrounds and
/// the words it shows. The review page marks a change with one circle around
/// the area that holds it, not one on each element in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CircleFields", into = "CircleFields")]
pub struct Circle {
    pub around: Around,
    pub label: Option<String>,
}

/// The area a circle surrounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Around {
    /// The elements a CSS selector finds on the board's pages, now and
    /// proposed: the page circles each one on each side where it finds one.
    /// A selector for a containing element surrounds the area it holds.
    Selector(String),
    /// A box on the board, on each side the page draws.
    Box(BoardBox),
}

/// A box on a board, in board pixels: its left and top edge, its width and
/// its height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardBox {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Circle {
    /// What the circle surrounds, as a person reads it.
    pub fn named(&self) -> String {
        match &self.around {
            Around::Selector(selector) => selector.clone(),
            Around::Box(b) => format!("box {},{} {}x{}", b.x, b.y, b.width, b.height),
        }
    }
}

/// A circle's JSON fields: a `selector` or a `box`, and a `label`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CircleFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selector: Option<String>,
    #[serde(default, rename = "box", skip_serializing_if = "Option::is_none")]
    area: Option<BoardBox>,
    #[serde(default)]
    label: Option<String>,
}

impl TryFrom<CircleFields> for Circle {
    type Error = String;

    /// Refuses a circle that gives both a selector and a box, or neither.
    fn try_from(fields: CircleFields) -> std::result::Result<Circle, String> {
        let around = match (fields.selector, fields.area) {
            (Some(selector), None) => Around::Selector(selector),
            (None, Some(area)) => Around::Box(area),
            _ => {
                return Err(
                    "a circle gives the `selector` of a containing element or a `box` (x, y, width, height) on the board, and one of them".to_string(),
                );
            }
        };
        Ok(Circle {
            around,
            label: fields.label,
        })
    }
}

impl From<Circle> for CircleFields {
    fn from(circle: Circle) -> CircleFields {
        let (selector, area) = match circle.around {
            Around::Selector(selector) => (Some(selector), None),
            Around::Box(area) => (None, Some(area)),
        };
        CircleFields {
            selector,
            area,
            label: circle.label,
        }
    }
}

/// The heading of a review page, the words under it, and the boards.
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
    /// changes the drawing of, and how it is resolved. A place has no
    /// resolution until the design session names its app work, which happens
    /// after the person approves the change; landing needs every place
    /// resolved (`check_changes`).
    pub places: BTreeMap<String, Option<Resolution>>,
    /// Each board place's canvas link, by the board's path; none in a
    /// project that publishes no canvas.
    pub boards: BTreeMap<String, Board>,
    /// The board files the change deletes: they need no answer or link, and
    /// the review page names them without a drawing.
    pub deleted: BTreeSet<String>,
    /// How the review page reads: its heading, words and boards. None for a
    /// change recorded as a fix or a decision, which has no page.
    pub review: Option<ReviewSection>,
    /// Why shared design files changed and no drawing did.
    pub no_app_place: Option<String>,
    /// What lets the change land: the person's review (`design approve`),
    /// or the recording of a fix or of a decided change. An amend clears it
    /// unless every board stays as the person approved it (`amend`).
    pub approved: Option<Stamp>,
    /// Whether the change waits on the person's review, or what settled it
    /// without one when it was recorded.
    pub settlement: Settlement,
    /// The session that recorded it, and whether the approval reached it.
    pub session: SessionState,
    /// The versions of one part of a board that its review page shows side
    /// by side (`design_variants`); none once it has closed.
    pub variants: Option<Variants>,
    pub opened: Stamp,
    pub landed: Vec<DesignLanding>,
}

/// What lets a design change land: the person's review of its page, or
/// what settled it without one, which approves it when it is recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Settlement {
    /// The change waits on the person's review of its page.
    Review,
    /// A fix that changes no design, and what it fixes: boards made to
    /// follow the design system's measures, a drawing mistake corrected, a
    /// board deleted because the app can no longer reach the state it draws,
    /// or a board redrawn to draw what the app draws today when the person
    /// decided nothing about the difference.
    Fix(String),
    /// A board edit of an existing screen that the design system's existing
    /// parts can draw, for a change the person decided in this decision of
    /// the log. The decision is the person's approval.
    Decision(Id),
}

impl Settlement {
    /// Does the change wait on the person's review of its page?
    pub fn needs_review(&self) -> bool {
        matches!(self, Settlement::Review)
    }
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
/// and `details` are required to record and optional to amend. Its JSON
/// gives `fix` or `decision`, or neither, which `RequestFields` reads.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RequestFields")]
pub struct DesignRequest {
    pub title: Option<String>,
    pub details: Option<String>,
    pub worktree: String,
    pub range: String,
    pub places: BTreeMap<String, Option<PlaceAnswer>>,
    pub boards: BTreeMap<String, Board>,
    pub review: Option<ReviewSection>,
    pub no_app_place: Option<String>,
    /// What settles the change: the person's review when the request gives
    /// neither a fix nor a decision.
    pub settlement: Settlement,
}

/// A design request's JSON fields, as the person or agent writes them.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestFields {
    pub title: Option<String>,
    pub details: Option<String>,
    /// A worktree of the repo the commits are in.
    pub worktree: String,
    /// The commits, e.g. `origin/main..HEAD`.
    pub range: String,
    /// The components a changed shared source changes the drawing of, each
    /// with `null`, and the app work of any place that has it: the range's
    /// changed places need no answer to record, and the design session gives
    /// each its answer after the person approves the change.
    #[serde(default)]
    pub places: BTreeMap<String, Option<PlaceAnswer>>,
    /// Each board place's canvas link, which a project that publishes a
    /// canvas gives for every board and any other project leaves out.
    #[serde(default)]
    pub boards: BTreeMap<String, Board>,
    /// How the review page reads: headings, the words under each, and its
    /// boards, which the dashboard draws from the range's last commit. Required
    /// to record; an amend that gives it replaces the page, one that doesn't
    /// keeps it. Every board place of the change must be on it.
    pub review: Option<ReviewSection>,
    /// Why shared design files changed and no drawing did.
    pub no_app_place: Option<String>,
    /// What the change fixes when it changes no design: boards made to follow
    /// the design system's measures, a drawing mistake corrected, a board
    /// deleted because the app can no longer reach the state it draws, or a
    /// board redrawn to draw what the app draws today when the person decided
    /// nothing about the difference. A fix
    /// needs no review page and is approved when recorded; an amend without
    /// it makes the change wait on the person's review again.
    pub fix: Option<String>,
    /// The decision of the log in which the person decided the change this
    /// board edit draws: a change to an existing screen that the design
    /// system's existing parts can draw. The decision is the approval, so
    /// the change needs no review page and is approved when recorded; an
    /// amend without it makes the change wait on the person's review again.
    pub decision: Option<Id>,
}

impl TryFrom<RequestFields> for DesignRequest {
    type Error = String;

    /// Refuses a request that gives both a fix and a decision, and a fix
    /// that says nothing.
    fn try_from(fields: RequestFields) -> std::result::Result<DesignRequest, String> {
        let settlement = match (fields.fix, fields.decision) {
            (Some(_), Some(_)) => {
                return Err("a design change is a `fix` that changes no design or a `decision` the person made, not both".to_string());
            }
            (Some(fixes), None) if fixes.trim().is_empty() => {
                return Err("fix is empty; say what the change fixes (the design system's measures, a drawing mistake, a board of a state the app can no longer reach, a board redrawn to draw what the app draws today)".to_string());
            }
            (Some(fixes), None) => Settlement::Fix(fixes),
            (None, Some(decision)) => Settlement::Decision(decision),
            (None, None) => Settlement::Review,
        };
        Ok(DesignRequest {
            title: fields.title,
            details: fields.details,
            worktree: fields.worktree,
            range: fields.range,
            places: fields.places,
            boards: fields.boards,
            review: fields.review,
            no_app_place: fields.no_app_place,
            settlement,
        })
    }
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
        let Some(answer) = answer else { continue };
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
    if let Some(section) = &mut design.review {
        section.boards.retain(|b| !dropped.contains(&b.board));
    }
    design.deleted = changes.deleted.clone();
    design
        .places
        .extend(resolutions.into_iter().map(|(path, how)| (path, Some(how))));
    for path in changed.iter().copied().chain(request.places.keys()) {
        design.places.entry(path.clone()).or_insert(None);
    }
    design.boards.extend(request.boards.clone());
    if let Some(review) = &request.review {
        design.review = Some(review.clone());
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
    if !missing.is_empty() && request.settlement.needs_review() {
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

/// The most words under a review page's heading: the person reads every
/// design change on it.
const MAX_SECTION_WORDS: usize = 25;

/// A review page's own rules: its heading has text, its words are at most
/// `MAX_SECTION_WORDS`, every board on it has a caption, and each circle has a
/// selector and, if it has a label, words in it. (That a board is a board file,
/// one of the repo's board places, and exists in the worktree is the command's,
/// which reads git and the files.)
fn check_review(section: &ReviewSection) -> Result<()> {
    if section.heading.trim().is_empty() {
        fail!("`review` has an empty heading");
    }
    let words = section.words.split_whitespace().count();
    if words > MAX_SECTION_WORDS {
        fail!(
            "the words of the review page are {words} words; they are at most {MAX_SECTION_WORDS}"
        );
    }
    for board in &section.boards {
        if board.caption.trim().is_empty() {
            fail!(
                "the board {} on the review page has an empty caption",
                board.board
            );
        }
        for circle in &board.circles {
            match &circle.around {
                Around::Selector(selector) if selector.trim().is_empty() => {
                    fail!(
                        "a circle on the board {} has an empty selector",
                        board.board
                    );
                }
                Around::Box(b) if b.width <= 0 || b.height <= 0 => {
                    fail!(
                        "the circle {} on the board {} has no width or no height; a box is at least 1 pixel each way",
                        circle.named(),
                        board.board
                    );
                }
                _ => {}
            }
            if circle.label.as_ref().is_some_and(|l| l.trim().is_empty()) {
                fail!(
                    "the circle {} on the board {} has an empty label; leave the label out or say what changed",
                    circle.named(),
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
            "{what} changes shared design-system sources ({}); list under `places` each component whose drawing changes (with `null` or its app work), or say with `no_app_place` that no drawing changes",
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
            Some(Resolution::Item(item)) => (Some(*item), None),
            Some(Resolution::Matches(how)) => (None, Some(how.as_str())),
            None => (None, None),
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
    if let Some(section) = &design.review {
        db.execute(
            "INSERT INTO review_section (design_id, heading, words) VALUES (?1, ?2, ?3)",
            params![design.id, section.heading, section.words],
        )
        .sql()?;
        for (at, board) in section.boards.iter().enumerate() {
            db.execute(
                "INSERT INTO review_board (design_id, position, board, caption, theme) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![design.id, at as i64, board.board, board.caption, board.theme.word()],
            )
            .sql()?;
            for (n, circle) in board.circles.iter().enumerate() {
                match &circle.around {
                    Around::Selector(selector) => db.execute(
                        "INSERT INTO review_circle (design_id, board, position, selector, label) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![design.id, at as i64, n as i64, selector, circle.label],
                    ),
                    Around::Box(b) => db.execute(
                        "INSERT INTO review_circle_box (design_id, board, position, x, y, width, height, label) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![design.id, at as i64, n as i64, b.x, b.y, b.width, b.height, circle.label],
                    ),
                }
                .sql()?;
            }
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
    /// when `tip` draws a place differently, and for a record.
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
    if request.review.is_none() && request.settlement.needs_review() {
        fail!(
            "a design change needs its `review`: the headings, words and boards of the page the person reviews it on (a `fix` that changes no design, or a board edit of a change the person decided in a `decision`, needs none)"
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
    keep_settled(db, design, request, stamp)?;
    Ok(Recorded { design, new_items })
}

/// Writes what settles change <id>, as <request> gives it: a fix or the
/// decision of a decided change approves the change as recorded by <stamp>,
/// and the person's review leaves it with neither. Refuses a decision id
/// that is no decision of the log.
fn keep_settled(db: &Connection, id: Id, request: &DesignRequest, stamp: &Stamp) -> Result<()> {
    db.execute("DELETE FROM design_fix WHERE design_id = ?1", [id])
        .sql()?;
    db.execute("DELETE FROM design_decision WHERE design_id = ?1", [id])
        .sql()?;
    match &request.settlement {
        Settlement::Review => Ok(()),
        Settlement::Fix(fixes) => {
            db.execute(
                "INSERT INTO design_fix (design_id, fixes) VALUES (?1, ?2)",
                params![id, fixes],
            )
            .sql()?;
            approve(db, id, stamp)
        }
        Settlement::Decision(decision) => {
            if !matches!(record::standing(db, *decision)?, Some((Kind::Decision, _))) {
                fail!(
                    "`decision` names {decision}, which is no decision in the log; give the id of the person's decision that settled the change"
                );
            }
            db.execute(
                "INSERT INTO design_decision (design_id, decision_id) VALUES (?1, ?2)",
                params![id, decision],
            )
            .sql()?;
            approve(db, id, stamp)
        }
    }
}

/// The open design change <id>.
pub(in crate::lists) fn open_design(lists: &Lists, id: Id) -> Result<&DesignChange> {
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
        && (before.settlement.needs_review() || !request.settlement.needs_review());
    if !approval_holds {
        db.execute("DELETE FROM design_approval WHERE design_id = ?1", [id])
            .sql()?;
    }
    keep_settled(db, id, request, stamp)?;
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
/// that `covers` checks it against (`Changes::drawn`, `drawn_by`).
pub struct DrawnChange<'a> {
    pub design: &'a DesignChange,
    pub drawing: Drawing,
}

/// What `covers` compares: design changes, each with how its last commit
/// draws some paths, and how the commits' last commit draws them.
pub struct Drawings<'a> {
    pub changes: Vec<DrawnChange<'a>>,
    pub now: Drawing,
}

impl<'a> Drawings<'a> {
    /// How each of <designs> and commit <last>, in the checkout at <top>,
    /// draw <paths>.
    pub fn read(
        top: &Path,
        last: &str,
        designs: impl IntoIterator<Item = &'a DesignChange>,
        paths: &BTreeSet<&String>,
    ) -> Result<Drawings<'a>> {
        let mut changes = Vec::new();
        for design in designs {
            changes.push(DrawnChange {
                design,
                drawing: drawn_by(top, design, paths.iter().copied())?,
            });
        }
        let now = drawing_at(top, last, paths.iter().copied())?;
        Ok(Drawings { changes, now })
    }
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
/// commits that are not its own. The error names each path that no change
/// covers. `builds design
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

/// Do the open design changes <designs> cover <changes> (`covers`), has the
/// person approved each, and does each place name its app work? `builds design check` runs this check, and
/// builds land runs that command before it lands design changes.
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
        let unnamed: Vec<&str> = design
            .places
            .iter()
            .filter(|(_, resolution)| resolution.is_none())
            .map(|(path, _)| path.as_str())
            .collect();
        if !unnamed.is_empty() {
            fail!(
                "design change {id} names no app work for {}; give each under `places` in `builds design amend {}` as `item` (an open item), `new` (an item draft) or `matches` (how it was checked), which keeps the person's approval",
                unnamed.join(", "),
                id.0
            );
        }
    }
    Ok(())
}

/// How design change <design> draws <paths>: as its last commit has them,
/// or not at all for a change that builds recorded before it began to keep
/// a change's last commit.
fn drawn_by<'a>(
    top: &Path,
    design: &DesignChange,
    paths: impl IntoIterator<Item = &'a String>,
) -> Result<Drawing> {
    match &design.tip {
        Some(tip) => drawing_at(top, tip, paths),
        None => Ok(Drawing::default()),
    }
}

/// One commit past origin/main that changes design files, and the id of the
/// open design change that covers it, if one does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesignCommit {
    pub sha: String,
    pub changes: Changes,
    pub covered_by: Option<Id>,
}

impl DesignCommit {
    /// The open design change that covers it, from <lists>, the lists it was
    /// read against or lists of the same version (`DesignReads`), which hold
    /// every change it names.
    pub fn covering<'a>(&self, lists: &'a Lists) -> Option<&'a DesignChange> {
        self.covered_by.map(|id| {
            lists
                .design(id)
                .expect("the lists a design commit was read against hold its covering change")
        })
    }
}

/// Each commit of <ahead> (the commits past origin/main, oldest first) in
/// the worktree at <top> of <repo> that changes design files, with the open
/// design change of the repo that covers it, approved or waiting on the
/// person's review. A change covers a commit as `covers` decides it
/// for `builds design check`, which land runs: it holds every place the
/// commit changes, and draws each changed path a drawing shows byte for byte
/// as the last of <ahead> does. The comparison reads files, so a change
/// still covers its commits after an agent rebases them onto a moved main.
/// Health and the dashboard's design card both read coverage here. Empty
/// when the lists name no design globs for the repo.
pub fn design_commits(
    lists: &Lists,
    repo: &str,
    top: &Path,
    ahead: &[String],
) -> Result<Vec<DesignCommit>> {
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
    let drawings = Drawings::read(top, last, open_designs(lists, repo), &drawn)?;
    Ok(commits
        .into_iter()
        .map(|(sha, changes)| {
            let covered_by = drawings
                .changes
                .iter()
                .find(|drawn| covers(std::slice::from_ref(*drawn), &changes, &drawings.now).is_ok())
                .map(|drawn| drawn.design.id);
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

/// Records that design change <id> landed as <commits> (full hashes),
/// closes it and deletes its variants. The same commits again are recorded once; other commits for a
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
    design_variants::clear(db, id)?;
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

/// Closes open design change <id> without landing it, records <why> as the
/// reason and deletes its variants. drop refuses a change that is already
/// closed, and one that an open item waits on: closing that change would
/// leave the item waiting on a change that never lands.
pub fn drop(db: &Connection, id: Id, why: &str, stamp: &Stamp) -> Result<()> {
    let lists = &store::read(db)?;
    open_design(lists, id)?;
    lists.require_none_waits_on(RecordWait::Design(id), "drop")?;
    record::close(db, id, How::Dropped(why.to_string()), stamp)?;
    design_variants::clear(db, id)?;
    Ok(())
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

fn describe(lists: &Lists, design: &DesignChange, closed: Option<&Closed>) -> Result<String> {
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
    match &design.settlement {
        Settlement::Review => {}
        Settlement::Fix(fixes) => {
            let _ = writeln!(out, "a fix that changes no design: {fixes}");
        }
        Settlement::Decision(decision) => {
            let title = &decided(lists, design.id, *decision)?.title;
            let _ = writeln!(
                out,
                "a board edit of a change the person decided in decision {decision}: {title}"
            );
        }
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
        let words = match resolution {
            Some(resolution) => resolution_words(lists, resolution),
            None => "no app work named yet".to_string(),
        };
        let _ = writeln!(out, "  {path}  {words}");
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
    if let Some(section) = &design.review {
        let _ = writeln!(out, "  review: {}", section.heading);
        for board in &section.boards {
            let _ = writeln!(
                out,
                "    {} ({}) {}",
                board.board,
                board.theme.word(),
                board.caption
            );
            for circle in &board.circles {
                match &circle.label {
                    Some(label) => {
                        let _ = writeln!(out, "      circles {}: {label}", circle.named());
                    }
                    None => {
                        let _ = writeln!(out, "      circles {}", circle.named());
                    }
                }
            }
        }
    }
    if let Some(variants) = &design.variants {
        out.push_str(&design_variants::describe(variants));
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
    Ok(out)
}

/// The decision <decision> that settled design change <design>. The table's
/// reference to the decision log keeps the decision there, so a change whose
/// decision the lists lack means a broken store, and this fails.
pub fn decided(lists: &Lists, design: Id, decision: Id) -> Result<&Decision> {
    match lists.decision(decision) {
        Some(found) => Ok(found),
        None => fail!(
            "design change {design} names decision {decision}, which the lists do not hold; the store is broken"
        ),
    }
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
        }) => describe(lists, lists.design(id).expect("open"), None),
        Some(Located {
            standing: Standing::Closed(closed),
            ..
        }) => {
            let Record::Design(design) = &closed.record else {
                unreachable!("located as a design change")
            };
            describe(lists, design, Some(closed))
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
    let mut places: BTreeMap<Id, BTreeMap<String, Option<Resolution>>> = BTreeMap::new();
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
            (Some(item), _) => Some(Resolution::Item(item)),
            (None, how) => how.map(Resolution::Matches),
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
    let mut review: BTreeMap<Id, ReviewSection> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT design_id, heading, words FROM review_section")
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
        review.insert(
            design,
            ReviewSection {
                heading,
                words,
                boards: Vec::new(),
            },
        );
    }
    // A board's circles in the order of their positions, which the two
    // tables share between them.
    let mut circles: BTreeMap<(Id, i64), Vec<(i64, Circle)>> = BTreeMap::new();
    let mut statement = db
        .prepare(
            "SELECT design_id, board, position, x, y, width, height, label FROM review_circle_box",
        )
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                BoardBox {
                    x: row.get(3)?,
                    y: row.get(4)?,
                    width: row.get(5)?,
                    height: row.get(6)?,
                },
                row.get::<_, Option<String>>(7)?,
            ))
        })
        .sql()?
    {
        let (design, board, position, area, label) = row.sql()?;
        circles.entry((design, board)).or_default().push((
            position,
            Circle {
                around: Around::Box(area),
                label,
            },
        ));
    }
    let mut statement = db
        .prepare("SELECT design_id, board, position, selector, label FROM review_circle")
        .sql()?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Id>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .sql()?
    {
        let (design, board, position, selector, label) = row.sql()?;
        circles.entry((design, board)).or_default().push((
            position,
            Circle {
                around: Around::Selector(selector),
                label,
            },
        ));
    }
    let mut statement = db
        .prepare("SELECT design_id, position, board, caption, theme FROM review_board ORDER BY design_id, position")
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
        let (design, position, board, caption, theme) = row.sql()?;
        let theme = Theme::of_word(&theme)?;
        review
            .get_mut(&design)
            .expect("review_board_is_on_a_section holds")
            .boards
            .push(ReviewBoard {
                board,
                caption,
                theme,
                circles: {
                    let mut held = circles.remove(&(design, position)).unwrap_or_default();
                    held.sort_by_key(|(at, _)| *at);
                    held.into_iter().map(|(_, circle)| circle).collect()
                },
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
    let mut decisions: BTreeMap<Id, Id> = BTreeMap::new();
    let mut statement = db
        .prepare("SELECT design_id, decision_id FROM design_decision")
        .sql()?;
    for row in statement
        .query_map([], |row| Ok((row.get::<_, Id>(0)?, row.get::<_, Id>(1)?)))
        .sql()?
    {
        let (design, decision) = row.sql()?;
        decisions.insert(design, decision);
    }
    let mut sessions = design_session::states(db)?;
    let mut variants = design_variants::all(db)?;
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
        let settlement = match (fixes.remove(&id), decisions.remove(&id)) {
            (Some(_), Some(_)) => {
                fail!("design change {id} is both a fix and a decided change; the store is broken")
            }
            (Some(fixes), None) => Settlement::Fix(fixes),
            (None, Some(decision)) => Settlement::Decision(decision),
            (None, None) => Settlement::Review,
        };
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
            review: review.remove(&id),
            no_app_place,
            approved: approvals.remove(&id),
            settlement,
            session: sessions.remove(&id).unwrap_or(SessionState::Unrecorded),
            variants: variants.remove(&id),
            opened: opened[&id].clone(),
            landed,
        });
    }
    Ok(designs)
}

/// Makes `design_place` again as design.sql writes it, which lets a place name
/// no app work yet, with every place it held. A store from before held every
/// place resolved.
pub fn allow_places_with_no_app_work(db: &Connection) -> Result<()> {
    let schema = include_str!("design.sql");
    crate::db::remake_table(
        db,
        "design_place",
        crate::db::statement(schema, "CREATE TABLE design_place", ";"),
        |db| {
            db.execute_batch(
                "INSERT INTO design_place (design_id, path, item_id, matches) SELECT design_id, path, item_id, matches FROM design_place_old",
            )
            .sql()
        },
        &[crate::db::statement(
            schema,
            "CREATE INDEX design_place_item",
            ";",
        )],
    )
}

/// The tables design.rs owns.
#[cfg(test)]
const TABLES: [&str; 13] = [
    "design",
    "design_session",
    "design_tip",
    "design_approval",
    "design_fix",
    "design_decision",
    "design_place",
    "design_board",
    "design_deleted",
    "review_section",
    "review_board",
    "review_circle",
    "review_circle_box",
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
