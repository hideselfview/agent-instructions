//! Boards and review pages. A board is a design file,
//! `design/mocks/<name>.dc.html`; `render_board` draws one as a page of HTML
//! the way the design canvas does: it follows the boards it mounts
//! (`dc-import`), applies each board's own values (`renderVals`), `sc-if` and
//! `sc-for`, and uses the worktree's design system: `bundle.js`, `bundle.css`,
//! and the `tokens.css` builds writes from `tokens.json`. The page's script
//! is `design_render/board.html`.
//!
//! `render_preview` draws a design-system component's `preview.html` the same
//! way, with the design system and React loaded ahead of its own script.
//!
//! A board is drawn from a `boards::Source`: a directory's files, or the
//! files at a commit, which git reads without checking anything out, so two
//! design changes can share a worktree and a change is drawn from its own
//! branch.
//!
//! A design change's review page (`design_render/review.html`) shows the
//! change's details, one point to a line, and the boards and component
//! previews its recorder picks to show the proposal (decision #369; a
//! component's preview is drawn by `preview_frame`, which the Designs page's
//! frame also draws, and reports its own size), in frames
//! that load the board's own
//! pages: proposed, the board at the last commit of the change's range, and,
//! beside it where the recorder asks, now, the board as it stands on
//! origin/main, each drawn with its own commit's design system. The page lists
//! the change's other boards by name, folded: each changed board opens to its
//! proposal, and each deleted board to its now. The daemon draws
//! each frame on every request, so the page shows a redraw on reload. The
//! sizes of the frames come from each commit's `design/mocks/canvas.json`; a
//! preview's frame starts at `PREVIEW_START` and takes the size the preview
//! reports: the whole preview in one column, as wide as its widest row asks,
//! with no scroll inside the frame. Where the page shows a preview now beside
//! its proposal, it levels each row with its twin (`preview_fit.js`).
//! The review page circles only what the designer says changed: it posts each
//! board's page the CSS selectors of the board's circles, the page answers
//! with the boxes of the elements each finds, and the review page circles
//! them on each side where they are (`design_render/circles.js`).

use std::collections::BTreeMap;
use std::path::Path;

use boards::{
    BOARD_SUFFIX, CANVAS, MOCKS, Source, board_name, board_path, canvas_size, imports, preview,
};
use serde_json::{Value, json};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::design::Circle;
use crate::lists::design_paths::PlaceKind;
use crate::lists::view::DesignView;
use crate::lists::{DesignChange, ReviewEntry, Shown, Theme};
use boards::Folders;

const BOARD_PAGE: &str = include_str!("design_render/board.html");
const REVIEW_PAGE: &str = include_str!("design_render/review.html");
/// The script that decides where the review page draws the designer's
/// circles (`design_render/circles.js`), which the review page runs.
const CIRCLES_SCRIPT: &str = include_str!("design_render/circles.js");

/// The script that lays a component's preview out in the room its frame has
/// and reports the size it takes (`design_render/preview_fit.js`), which the
/// Designs page and the review page run in a preview's frame.
const PREVIEW_FIT: &str = include_str!("design_render/preview_fit.js");
/// The script that answers the review page's question for the boxes of the
/// elements a selector finds (`design_render/circle_answers.js`), which a
/// board's page and a preview's frame run.
const CIRCLE_ANSWERS: &str = include_str!("design_render/circle_answers.js");
/// What a preview's frame runs on a review page to answer that question
/// (`design_render/preview_circles.js`).
const PREVIEW_CIRCLES: &str = include_str!("design_render/preview_circles.js");

/// The script tags that load React 18.3.1, which a board page and a
/// component's preview page draw with.
pub(crate) const REACT: &str = r#"<script src="https://cdnjs.cloudflare.com/ajax/libs/react/18.3.1/umd/react.production.min.js"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/react-dom/18.3.1/umd/react-dom.production.min.js"></script>"#;

/// The script that draws a board's page in a frame at the board's own size,
/// scaled (`design_render/frames.js`), which the review page, the
/// dashboard's Designs page and its scratch view run.
pub const FRAMES_SCRIPT: &str = include_str!("design_render/frames.js");
/// The look of a page that shows boards in frames: the review page and the
/// dashboard's scratch view (`design_render/frames.css`).
pub const FRAMES_CSS: &str = include_str!("design_render/frames.css");

/// What a board with no script of its own runs.
const NO_SCRIPT: &str =
    "class Component extends DCLogic { renderVals() { return { yes: true }; } }";

/// <template> with each `@@NAME@@` replaced by its part, in one pass: a part
/// holding such a marker is not filled again.
pub fn fill(template: &str, parts: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("@@") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let found = after.find("@@").and_then(|end| {
            let name = &after[..end];
            parts
                .iter()
                .find(|(part, _)| *part == name)
                .map(|(_, value)| (*value, &after[end + 2..]))
        });
        match found {
            Some((value, next)) => {
                out.push_str(value);
                rest = next;
            }
            None => {
                out.push_str("@@");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The revision the boards of "now" are drawn at.
pub const NOW: &str = "origin/main";

/// What a board file holds that the page needs.
struct Part {
    /// The markup between `<x-dc>` and `</x-dc>`, less its `<helmet>`.
    body: String,
    /// The first `<style>` of the markup (the helmet's usually).
    style: String,
    script: String,
}

/// The text between the first <open> of <text> and the <close> after it.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

fn parse_part(from: &Source, rel: &str) -> Result<Part> {
    let path = from.label(rel);
    let source = from.read(rel, "the board")?;
    let (Some(open), Some(close)) = (source.find("<x-dc>"), source.rfind("</x-dc>")) else {
        fail!("{path} has no <x-dc>...</x-dc>");
    };
    if close < open {
        fail!("{path} closes its <x-dc> before it opens");
    }
    let mut body = source[open + "<x-dc>".len()..close].to_string();
    let style = between(&body, "<style>", "</style>")
        .unwrap_or("")
        .to_string();
    while let Some(start) = body.find("<helmet>") {
        let Some(end) = body[start..].find("</helmet>") else {
            fail!("{path} has a <helmet> that never closes")
        };
        body.replace_range(start..start + end + "</helmet>".len(), "");
    }
    let script = source
        .find("<script type=\"text/x-dc\"")
        .and_then(|start| {
            let tag_end = source[start..].find('>')? + start + 1;
            let end = source[tag_end..].find("</script>")? + tag_end;
            Some(source[tag_end..end].to_string())
        })
        .unwrap_or_else(|| NO_SCRIPT.to_string());
    Ok(Part {
        body,
        style,
        script,
    })
}

/// Inline in a `<script>`: the one string that would end it early is written
/// so that it doesn't, which a script's strings read as the same text.
pub(crate) fn script_safe(js: &str) -> String {
    js.replace("</script", "<\\/script")
}

/// Board <board> of <from> as one page of HTML in <theme>, drawn with the
/// design system of <from>; a commit's is generated into <cache>
/// (`Folders::cache`).
pub fn render_board(from: Source, board: &str, theme: Theme, dirs: &Folders) -> Result<String> {
    from.check()?;
    let name = board_name(board)?;
    let mut parts: BTreeMap<String, Part> = BTreeMap::new();
    let mut todo = vec![("__main__".to_string(), board_path(&name))];
    while let Some((key, rel)) = todo.pop() {
        let part = parse_part(&from, &rel)?;
        for dep in imports(&part.body) {
            let Ok(dep_name) = board_name(&dep) else {
                fail!(
                    "{} mounts {dep:?}, which is no board name",
                    from.label(&rel)
                )
            };
            if !parts.contains_key(&dep_name) && !todo.iter().any(|(k, _)| *k == dep_name) {
                if !from.has(&board_path(&dep_name))? {
                    fail!(
                        "{} mounts the board {dep_name}, which is no file in {}",
                        from.label(&rel),
                        from.label(MOCKS)
                    );
                }
                todo.push((dep_name.clone(), board_path(&dep_name)));
            }
        }
        parts.insert(key, part);
    }
    let [tokens_css, bundle_css, bundle_js] = from.board_design_system(dirs)?;
    let styles: String = parts.values().map(|p| p.style.as_str()).collect();
    let templates: String = parts
        .iter()
        .map(|(k, p)| format!("<template id=\"t-{k}\">{}</template>\n", p.body))
        .collect();
    let classes: String = parts
        .iter()
        .map(|(k, p)| {
            format!(
                "C[\"{k}\"] = (function () {{ {} ; return Component; }})();\n",
                script_safe(&p.script)
            )
        })
        .collect();
    Ok(fill(
        BOARD_PAGE,
        &[
            ("THEME", theme.word()),
            ("REACT", REACT),
            ("TOKENS_CSS", &tokens_css),
            ("BUNDLE_CSS", &bundle_css),
            ("BUNDLE_JS", &script_safe(&bundle_js)),
            ("STYLES", &styles),
            ("TEMPLATES", &templates),
            ("CLASSES", &classes),
            ("CIRCLE_ANSWERS", CIRCLE_ANSWERS),
        ],
    ))
}

/// <value> as JSON to put inline in a `<script>`: `<` never appears in it as
/// itself, so no text in it can end the script.
pub fn script_json(value: &Value) -> String {
    value.to_string().replace('<', "\\u003c")
}

/// Component <name>'s preview page of <from>, with the design system of
/// <from> and React loaded ahead of its own script, as the Design System
/// page draws its card; a commit's design system is generated into <cache>.
pub fn render_preview(from: Source, name: &str, dirs: &Folders) -> Result<String> {
    from.check()?;
    if !preview::is_component_name(name) {
        fail!("{name:?} is no component: a component's name is letters and digits");
    }
    let rel = preview::preview_path(name);
    let html = from.read(&rel, "the preview")?;
    let [tokens_css, bundle_css, bundle_js] = from.board_design_system(dirs)?;
    let loads = format!(
        "<style>{tokens_css}</style><style>{bundle_css}</style>{REACT}<script>{}</script>",
        script_safe(&bundle_js)
    );
    match preview::with_loads(&html, &loads) {
        Some(page) => Ok(page),
        None => fail!(
            "{} has no <head> to load the design system in",
            from.label(&rel)
        ),
    }
}

/// Component <name>'s preview of <from> as a frame loads it: the preview
/// page (`render_preview`) with `PREVIEW_FIT`, which draws it at its own size
/// and tells the page that size. A frame of a review page (<for_review>) also
/// answers the page's question for the boxes of the elements a selector finds.
pub fn preview_frame(from: Source, name: &str, dirs: &Folders, for_review: bool) -> Result<String> {
    let page = render_preview(from, name, dirs)?;
    let mut loads = format!("<script>{PREVIEW_FIT}</script>");
    if for_review {
        loads.push_str(&format!(
            "<script>{CIRCLE_ANSWERS}</script><script>{PREVIEW_CIRCLES}</script>"
        ));
    }
    Ok(preview::with_loads(&page, &loads).expect("render_preview's page has a <head>"))
}

/// The <n>th entry of <design>'s review page: a board or a component's
/// preview.
pub fn review_board(design: &DesignChange, n: usize) -> Option<&ReviewEntry> {
    design.review.as_ref()?.boards.get(n)
}

/// The board places of <design> that its review page does not show, in path
/// order: the page lists them by name, and each opens to its proposal.
fn other_boards(design: &DesignChange) -> impl Iterator<Item = &String> {
    let shown: Vec<&str> = design
        .review
        .iter()
        .flat_map(|section| &section.boards)
        .filter_map(|entry| match &entry.shown {
            Shown::Board { path, .. } => Some(path.as_str()),
            Shown::Component(_) => None,
        })
        .collect();
    design.places.keys().filter(move |path| {
        PlaceKind::of(path) == PlaceKind::Board && !shown.contains(&path.as_str())
    })
}

/// The <k>th board place of <design> that its review page does not show
/// (`other_boards`).
pub fn other_board(design: &DesignChange, k: usize) -> Option<&String> {
    other_boards(design).nth(k)
}

/// The boards <design> deletes, in path order. A deleted component has no
/// board to show.
fn deleted_boards(design: &DesignChange) -> impl Iterator<Item = &String> {
    design
        .deleted
        .iter()
        .filter(|place| PlaceKind::of(place) == PlaceKind::Board)
}

/// The <k>th board <design> deletes, in path order.
pub fn deleted_board(design: &DesignChange, k: usize) -> Option<&String> {
    deleted_boards(design).nth(k)
}

/// Which drawing of a board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The board as it stands on origin/main.
    Now,
    /// The board at the last commit of the change's range.
    Proposed,
}

impl Side {
    pub fn word(self) -> &'static str {
        match self {
            Side::Now => "now",
            Side::Proposed => "proposed",
        }
    }
}

/// Where <side>'s files of <design> are read from. The change's repo is read
/// through its worktree, whatever branch the worktree has checked out.
pub fn source_of(design: &DesignChange, side: Side) -> Result<Source<'_>> {
    let repo = Path::new(&design.worktree);
    match (side, &design.tip) {
        (Side::Now, _) => Ok(Source::Commit { repo, rev: NOW }),
        (Side::Proposed, Some(tip)) => Ok(Source::Commit { repo, rev: tip }),
        (Side::Proposed, None) => fail!(
            "design change {} was recorded before its last commit was kept; amend it to draw its boards",
            design.id.0
        ),
    }
}

/// The size that the canvas file of <from> gives board <name>.
fn size_at(from: &Source, name: &str) -> Result<(u64, u64)> {
    let text = from.read(CANVAS, "the canvas file")?;
    let canvas: Value =
        serde_json::from_str(&text).context(|| format!("reading {}", from.label(CANVAS)))?;
    Ok(canvas_size(&canvas, &format!("{name}{BOARD_SUFFIX}"))?)
}

/// The size that the canvas file of <from> gives board <board> (a name or a
/// path), or None when the board is no file there.
pub fn board_size(from: &Source, board: &str) -> Result<Option<(u64, u64)>> {
    from.check()?;
    let name = board_name(board)?;
    if !from.has(&board_path(&name))? {
        return Ok(None);
    }
    Ok(Some(size_at(from, &name)?))
}

/// How <side> of board <board> shows on the review page: its frame (`w`, `h`
/// and the `src` the frame loads), `absent` (the board is no file there), or
/// the `error` that stops it.
fn drawing(design: &DesignChange, side: Side, board: &str, src: &str) -> Value {
    match source_of(design, side).and_then(|from| board_size(&from, board)) {
        Ok(Some((w, h))) => json!({ "w": w, "h": h, "src": format!("{src}/{}", side.word()) }),
        // A board that is no file where the change draws it is that change's fault, not an absence.
        Ok(None) if side == Side::Proposed => {
            json!({ "error": format!("{board} is no file at the change's last commit") })
        }
        Ok(None) => json!({ "absent": true }),
        Err(error) => json!({ "error": error.text() }),
    }
}

/// The size a component's preview frame starts at on the review page,
/// before the preview reports the size it takes (`preview_fit.js`).
const PREVIEW_START: (u64, u64) = (480, 320);

/// How <side> of component <name>'s preview shows on the review page: its
/// frame (`w`, `h`, `src` and `preview`, which tells the page that the
/// preview reports its own size), `absent` (the component has no preview
/// there), or the `error` that stops it.
fn preview_drawing(design: &DesignChange, side: Side, name: &str, src: &str) -> Value {
    let found =
        source_of(design, side).and_then(|from| Ok(from.has(&preview::preview_path(name))?));
    match found {
        Ok(true) => json!({
            "w": PREVIEW_START.0,
            "h": PREVIEW_START.1,
            "preview": true,
            "src": format!("{src}/{}", side.word()),
        }),
        // A component with no preview where the change draws it is that change's fault, not an absence.
        Ok(false) if side == Side::Proposed => {
            json!({ "error": format!("{name} has no preview at the change's last commit") })
        }
        Ok(false) => json!({ "absent": true }),
        Err(error) => json!({ "error": error.text() }),
    }
}

/// A circle as the review page reads it: its `selector` or `box`, its `label`
/// and its `name`, which the page shows when the circle has no label.
fn circle_json(circle: &Circle) -> Value {
    let mut value = serde_json::to_value(circle).expect("a circle is JSON");
    value["name"] = json!(circle.named());
    value
}

/// <design>'s review page. <view> is the design as the dashboard's lists JSON
/// has it, which holds the app work its places resolve to, and <scratch> the
/// scratch boards in the change's worktree that the page links to
/// (`Model::scratch_boards`). A board whose size or file can't be read shows
/// as that problem in its place, not as a page that fails whole.
pub fn review_page(
    design: &DesignChange,
    view: &DesignView,
    project: &str,
    scratch: Option<&crate::daemon::scratch_view::Scratch>,
) -> Result<String> {
    let section = design.review.as_ref().map(|section| {
        let boards: Vec<Value> = section
            .boards
            .iter()
            .enumerate()
            .map(|(n, entry)| {
                let src = format!("/design/{project}/{}/board/{n}", design.id.0);
                let (mut shown, draw): (Value, Box<dyn Fn(Side) -> Value>) = match &entry.shown {
                    Shown::Board { path, theme } => (
                        json!({
                            "path": path,
                            "theme": theme.word(),
                        }),
                        Box::new(|side| drawing(design, side, path, &src)),
                    ),
                    Shown::Component(name) => (
                        json!({ "component": name }),
                        Box::new(|side| preview_drawing(design, side, name, &src)),
                    ),
                };
                shown["caption"] = json!(entry.caption);
                shown["circles"] = entry.circles.iter().map(circle_json).collect();
                shown["proposed"] = draw(Side::Proposed);
                if entry.show_now {
                    shown["now"] = draw(Side::Now);
                }
                shown
            })
            .collect();
        json!({ "boards": boards })
    });
    let others: Vec<Value> = other_boards(design)
        .enumerate()
        .map(|(k, path)| {
            let src = format!("/design/{project}/{}/changed/{k}", design.id.0);
            json!({ "path": path, "proposed": drawing(design, Side::Proposed, path, &src) })
        })
        .collect();
    let deleted: Vec<Value> = deleted_boards(design)
        .enumerate()
        .map(|(k, path)| {
            let src = format!("/design/{project}/{}/deleted/{k}", design.id.0);
            // A deleted board is a file on origin/main, so it is never `absent` there.
            json!({ "path": path, "now": drawing(design, Side::Now, path, &src) })
        })
        .collect();
    let mut data = serde_json::to_value(view).expect("the view serializes");
    data["section"] = section.unwrap_or(Value::Null);
    data["others"] = Value::Array(others);
    data["deleted"] = Value::Array(deleted);
    data["scratch"] = serde_json::to_value(scratch).expect("the scratch boards serialize");
    Ok(fill(
        REVIEW_PAGE,
        &[
            ("FRAMES_CSS", FRAMES_CSS),
            ("DATA", &script_json(&data)),
            ("CIRCLES", CIRCLES_SCRIPT),
            ("FRAMES", FRAMES_SCRIPT),
        ],
    ))
}

#[cfg(test)]
#[path = "design_render_tests.rs"]
mod tests;
