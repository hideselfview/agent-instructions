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
//! change's words under their heading, and each board twice, side by side in
//! frames that load the board's own pages: now, the board as it stands on
//! origin/main, and proposed, the board at the last commit of the change's
//! range, each drawn with its own commit's design system. A board the change
//! adds has no now, and a board it deletes has only a now. The daemon draws
//! each frame on every request, so the page shows a redraw on reload. The
//! sizes of the frames come from each commit's `design/mocks/canvas.json`.
//! The review page circles only what the designer says changed: it posts each
//! board's page the CSS selectors of the board's circles, the page answers
//! with the boxes of the elements each finds, and the review page circles
//! them on each side where they are (`design_render/circles.js`).
//!
//! A board takes props as a board that mounts it passes its attributes: its
//! script reads them from `this.props`. A change's variants
//! (`design_variants`) draw one board once per set of props, at the commit
//! the variants keep, and the review page shows them side by side, each
//! cropped to the first element the variants' selector finds there: it asks
//! each variant's page what the selector finds, as it asks for the circles,
//! and crops the frame to it (`design_render/variants.js`).

use std::collections::BTreeMap;
use std::path::Path;

use boards::{
    BOARD_SUFFIX, CANVAS, MOCKS, Source, board_name, board_path, canvas_size, imports, preview,
};
use serde_json::{Value, json};

use crate::fail;
use crate::fail::{Context, Result};
use crate::lists::design::Circle;
use crate::lists::design_variants::{self, Props, Variants};
use crate::lists::view::DesignView;
use crate::lists::{DesignChange, ReviewBoard, Theme};

const BOARD_PAGE: &str = include_str!("design_render/board.html");
const REVIEW_PAGE: &str = include_str!("design_render/review.html");
/// The script that decides where the review page draws the designer's
/// circles (`design_render/circles.js`), which the review page runs.
const CIRCLES_SCRIPT: &str = include_str!("design_render/circles.js");
/// The script that decides where the review page crops each drawing of a
/// change's variants (`design_render/variants.js`), which the review page
/// runs.
const VARIANTS_SCRIPT: &str = include_str!("design_render/variants.js");

/// The script tags that load React 18.3.1, which a board page and a
/// component's preview page draw with.
const REACT: &str = r#"<script src="https://cdnjs.cloudflare.com/ajax/libs/react/18.3.1/umd/react.production.min.js"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/react-dom/18.3.1/umd/react-dom.production.min.js"></script>"#;

/// The script that draws a board's page in a frame at the board's own size,
/// scaled (`design_render/frames.js`), which the review page and the
/// dashboard's Designs page run.
pub const FRAMES_SCRIPT: &str = include_str!("design_render/frames.js");

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
fn script_safe(js: &str) -> String {
    js.replace("</script", "<\\/script")
}

/// Board <board> of <from> as one page of HTML in <theme>, drawn with
/// <props> and the design system of <from>; a commit's is generated into
/// <cache> (`Places::design_system_cache`).
pub fn render_board(
    from: Source,
    board: &str,
    theme: Theme,
    props: &Props,
    cache: &Path,
) -> Result<String> {
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
    let [tokens_css, bundle_css, bundle_js] = from.design_system(cache)?;
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
            ("PROPS", &script_json(&Value::Object(props.clone()))),
        ],
    ))
}

/// <value> as JSON to put inline in a `<script>`: `<` never appears in it as
/// itself, so no text in it can end the script.
fn script_json(value: &Value) -> String {
    value.to_string().replace('<', "\\u003c")
}

/// Component <name>'s preview page of <from>, with the design system of
/// <from> and React loaded ahead of its own script, as the Design System
/// page draws its card; a commit's design system is generated into <cache>.
pub fn render_preview(from: Source, name: &str, cache: &Path) -> Result<String> {
    from.check()?;
    if !preview::is_component_name(name) {
        fail!("{name:?} is no component: a component's name is letters and digits");
    }
    let rel = preview::preview_path(name);
    let html = from.read(&rel, "the preview")?;
    let [tokens_css, bundle_css, bundle_js] = from.design_system(cache)?;
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

/// The <n>th board of <design>'s review page.
pub fn review_board(design: &DesignChange, n: usize) -> Option<&ReviewBoard> {
    design.review.as_ref()?.boards.get(n)
}

/// The <k>th board <design> deletes, in path order.
pub fn deleted_board(design: &DesignChange, k: usize) -> Option<&String> {
    design.deleted.iter().nth(k)
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

/// How <side> of board <board> shows on the review page: its frame (`w`, `h`
/// and the `src` the frame loads), `absent` (the board is no file there), or
/// the `error` that stops it.
fn drawing(design: &DesignChange, side: Side, board: &str, src: &str) -> Value {
    let drawn = (|| -> Result<Value> {
        let from = source_of(design, side)?;
        from.check()?;
        let name = board_name(board)?;
        if !from.has(&board_path(&name))? {
            return Ok(json!({ "absent": true }));
        }
        let text = from.read(CANVAS, "the canvas file")?;
        let canvas: Value =
            serde_json::from_str(&text).context(|| format!("reading {}", from.label(CANVAS)))?;
        let (w, h) = canvas_size(&canvas, &format!("{name}{BOARD_SUFFIX}"))?;
        Ok(json!({ "w": w, "h": h, "src": format!("{src}/{}", side.word()) }))
    })();
    // A board that is no file where the change draws it is that change's fault, not an absence.
    match drawn {
        Ok(value) if side == Side::Proposed && value.get("absent").is_some() => {
            json!({ "error": format!("{board} is no file at the change's last commit") })
        }
        Ok(value) => value,
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
/// has it, which holds the app work its places resolve to. A board whose size
/// or file can't be read shows as that problem in its place, not as a page
/// that fails whole.
pub fn review_page(design: &DesignChange, view: &DesignView, project: &str) -> Result<String> {
    let section = design.review.as_ref().map(|section| {
        let boards: Vec<Value> = section
            .boards
            .iter()
            .enumerate()
            .map(|(n, board)| {
                let src = format!("/design/{project}/{}/board/{n}", design.id.0);
                json!({
                    "path": board.board,
                    "caption": board.caption,
                    "theme": board.theme.word(),
                    "circles": board.circles.iter().map(circle_json).collect::<Vec<_>>(),
                    "link": design.boards.get(&board.board).map(|b| b.link.clone()),
                    "now": drawing(design, Side::Now, &board.board, &src),
                    "proposed": drawing(design, Side::Proposed, &board.board, &src),
                })
            })
            .collect();
        json!({ "heading": section.heading, "words": section.words, "boards": boards })
    });
    let deleted: Vec<Value> = design
        .deleted
        .iter()
        .enumerate()
        .map(|(k, path)| {
            let src = format!("/design/{project}/{}/deleted/{k}", design.id.0);
            // A deleted board is a file on origin/main, so it is never `absent` there.
            json!({ "path": path, "now": drawing(design, Side::Now, path, &src) })
        })
        .collect();
    let mut data = serde_json::to_value(view).expect("the view serializes");
    data["section"] = section.unwrap_or(Value::Null);
    data["deleted"] = Value::Array(deleted);
    data["variants"] = design.variants.as_ref().map_or(Value::Null, |variants| {
        variants_json(design, variants, project)
    });
    Ok(fill(
        REVIEW_PAGE,
        &[
            ("DATA", &script_json(&data)),
            ("CIRCLES", CIRCLES_SCRIPT),
            ("VARIANTS", VARIANTS_SCRIPT),
            ("FRAMES", FRAMES_SCRIPT),
        ],
    ))
}

/// Where <variants> of <design> draw their board: at their commit, in the
/// change's worktree, whatever branch it has checked out.
pub fn variants_source<'a>(design: &'a DesignChange, variants: &'a Variants) -> Source<'a> {
    Source::Commit {
        repo: Path::new(&design.worktree),
        rev: &variants.tip,
    }
}

/// <variants> as the review page reads them: the board, the selector, the
/// theme, the commit, and each drawing with its label and its frame (`w`,
/// `h` and the `src` it loads) or the `error` that stops it, which the page
/// shows in its place.
fn variants_json(design: &DesignChange, variants: &Variants, project: &str) -> Value {
    let size = (|| -> Result<(u64, u64)> {
        let from = variants_source(design, variants);
        from.check()?;
        let name = board_name(&variants.board)?;
        if !from.has(&board_path(&name))? {
            fail!("{} is no file at commit {}", variants.board, variants.tip);
        }
        let text = from.read(CANVAS, "the canvas file")?;
        let canvas: Value =
            serde_json::from_str(&text).context(|| format!("reading {}", from.label(CANVAS)))?;
        Ok(canvas_size(&canvas, &format!("{name}{BOARD_SUFFIX}"))?)
    })()
    .map_err(crate::fail::Fail::text);
    let drawings: Vec<Value> = variants
        .props
        .iter()
        .enumerate()
        .map(|(k, props)| {
            let label = design_variants::label(props);
            match &size {
                Ok((w, h)) => json!({
                    "label": label,
                    "w": w,
                    "h": h,
                    "src": format!("/design/{project}/{}/variant/{k}", design.id.0),
                }),
                Err(error) => json!({ "label": label, "error": error }),
            }
        })
        .collect();
    json!({
        "board": variants.board,
        "selector": variants.selector,
        "theme": variants.theme.word(),
        "tip": variants.tip,
        "drawings": drawings,
    })
}

#[cfg(test)]
#[path = "design_render_tests.rs"]
mod tests;
