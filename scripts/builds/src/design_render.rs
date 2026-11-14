//! Boards and review pages. A board is a design file,
//! `design/mocks/<name>.dc.html`; `render_board` draws one as a page of HTML
//! the way the design canvas does: it follows the boards it mounts
//! (`dc-import`), applies each board's own values (`renderVals`), `sc-if` and
//! `sc-for`, and uses the worktree's design system (`bundle.js`, `bundle.css`,
//! `tokens.css`). The page's script is `design_render/board.html`.
//!
//! A design change's review page (`design_render/review.html`) shows the
//! change's words under their headings with each board in a frame that loads
//! the board's own page, so the daemon draws it from the worktree on every
//! request and the page shows a redraw on reload. The sizes of the frames come
//! from the worktree's `design/mocks/canvas.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::lists::view::DesignView;
use crate::lists::{DesignChange, ReviewBoard, Theme};

const BOARD_PAGE: &str = include_str!("design_render/board.html");
const REVIEW_PAGE: &str = include_str!("design_render/review.html");

const MOCKS: &str = "design/mocks";
const BOARD_SUFFIX: &str = ".dc.html";
/// What a board with no script of its own runs.
const NO_SCRIPT: &str =
    "class Component extends DCLogic { renderVals() { return { yes: true }; } }";

/// <template> with each `@@NAME@@` replaced by its part, in one pass: a part
/// holding such a marker is not filled again.
fn fill(template: &str, parts: &[(&str, &str)]) -> String {
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

/// A board's name, from its name or its path `design/mocks/<name>.dc.html`.
/// A name is one file's, so nothing in it leaves the folder.
pub fn board_name(board: &str) -> Result<String> {
    let name = board.strip_prefix(&format!("{MOCKS}/")).unwrap_or(board);
    let name = name.strip_suffix(BOARD_SUFFIX).unwrap_or(name);
    let fine = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if !fine {
        fail!(
            "{board:?} is no board: a board is design/mocks/<name>.dc.html, its name letters, digits, `.`, `-` and `_`"
        );
    }
    Ok(name.to_string())
}

fn board_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(MOCKS).join(format!("{name}{BOARD_SUFFIX}"))
}

fn read(path: &Path, what: &str) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fail!("{what} {} is not there", path.display())
        }
        Err(error) => fail!("reading {what} {}: {error}", path.display()),
    }
}

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

fn parse_part(path: &Path) -> Result<Part> {
    let source = read(path, "the board")?;
    let (Some(open), Some(close)) = (source.find("<x-dc>"), source.rfind("</x-dc>")) else {
        fail!("{} has no <x-dc>...</x-dc>", path.display());
    };
    if close < open {
        fail!("{} closes its <x-dc> before it opens", path.display());
    }
    let mut body = source[open + "<x-dc>".len()..close].to_string();
    let style = between(&body, "<style>", "</style>")
        .unwrap_or("")
        .to_string();
    while let Some(start) = body.find("<helmet>") {
        let Some(end) = body[start..].find("</helmet>") else {
            fail!("{} has a <helmet> that never closes", path.display())
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

/// The names of the boards <body> mounts.
fn imports(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<dc-import name=\"") {
        let after = &rest[start + "<dc-import name=\"".len()..];
        let Some(end) = after.find('"') else { break };
        names.push(after[..end].to_string());
        rest = &after[end..];
    }
    names
}

/// Inline in a `<script>`: the one string that would end it early is written
/// so that it doesn't, which a script's strings read as the same text.
fn script_safe(js: &str) -> String {
    js.replace("</script", "<\\/script")
}

/// Board <board> of worktree <dir> as one page of HTML in <theme>.
pub fn render_board(dir: &Path, board: &str, theme: Theme) -> Result<String> {
    let name = board_name(board)?;
    let mocks = dir.join(MOCKS);
    let mut parts: BTreeMap<String, Part> = BTreeMap::new();
    let mut todo = vec![("__main__".to_string(), board_path(dir, &name))];
    while let Some((key, path)) = todo.pop() {
        let part = parse_part(&path)?;
        for dep in imports(&part.body) {
            let Ok(dep_name) = board_name(&dep) else {
                fail!("{} mounts {dep:?}, which is no board name", path.display())
            };
            if !parts.contains_key(&dep_name) && !todo.iter().any(|(k, _)| *k == dep_name) {
                if !mocks.join(format!("{dep_name}{BOARD_SUFFIX}")).is_file() {
                    fail!(
                        "{} mounts the board {dep_name}, which is no file in {}",
                        path.display(),
                        mocks.display()
                    );
                }
                todo.push((dep_name.clone(), board_path(dir, &dep_name)));
            }
        }
        parts.insert(key, part);
    }
    let system = dir.join("design/system");
    let tokens_css = read(&system.join("tokens.css"), "the design system's")?;
    let bundle_css = read(&system.join("components/bundle.css"), "the design system's")?;
    let bundle_js = read(&system.join("components/bundle.js"), "the design system's")?;
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
            ("TOKENS_CSS", &tokens_css),
            ("BUNDLE_CSS", &bundle_css),
            ("BUNDLE_JS", &script_safe(&bundle_js)),
            ("STYLES", &styles),
            ("TEMPLATES", &templates),
            ("CLASSES", &classes),
        ],
    ))
}

/// The width and height <canvas> (`canvas.json`) gives board <name>.
fn canvas_size(canvas: &Value, name: &str) -> Result<(u64, u64)> {
    let board = &canvas["boards"][format!("{name}{BOARD_SUFFIX}")];
    match (board["w"].as_u64(), board["h"].as_u64()) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Ok((w, h)),
        _ => fail!("design/mocks/canvas.json gives no width and height for {name}{BOARD_SUFFIX}"),
    }
}

/// The <n>th board of <design>'s review page, counting through its sections
/// in order.
pub fn review_board(design: &DesignChange, n: usize) -> Option<&ReviewBoard> {
    design
        .review
        .iter()
        .flat_map(|section| &section.boards)
        .nth(n)
}

/// <design>'s review page. <view> is the design as the dashboard's lists JSON
/// has it, which holds the app work its places resolve to. A board whose size
/// or file can't be read shows as that problem in its place, not as a page
/// that fails whole.
pub fn review_page(design: &DesignChange, view: &DesignView) -> Result<String> {
    let dir = Path::new(&design.worktree);
    let canvas_path = dir.join(MOCKS).join("canvas.json");
    let canvas: std::result::Result<Value, String> = read(&canvas_path, "the canvas file")
        .and_then(|text| {
            serde_json::from_str(&text).context(|| format!("reading {}", canvas_path.display()))
        })
        .map_err(Fail::text);
    let mut n = 0;
    let mut sections = Vec::new();
    for section in &design.review {
        let mut boards = Vec::new();
        for board in &section.boards {
            let shown = board_name(&board.board)
                .and_then(|name| {
                    let (w, h) = canvas_size(canvas.as_ref().map_err(|e| Fail::Message(e.clone()))?, &name)?;
                    if !board_path(dir, &name).is_file() {
                        fail!("{} is no file in {}", board.board, dir.display());
                    }
                    Ok((w, h))
                })
                .map(|(w, h)| json!({ "w": w, "h": h, "src": format!("/design/{}/board/{n}", design.id.0) }))
                .unwrap_or_else(|error| json!({ "error": error.text() }));
            let mut entry = json!({
                "path": board.board,
                "caption": board.caption,
                "theme": board.theme.word(),
                "link": design.boards.get(&board.board).map(|b| b.link.clone()),
            });
            entry
                .as_object_mut()
                .expect("an object")
                .extend(shown.as_object().expect("an object").clone());
            boards.push(entry);
            n += 1;
        }
        sections
            .push(json!({ "heading": section.heading, "words": section.words, "boards": boards }));
    }
    let mut data = serde_json::to_value(view).expect("the view serializes");
    data["sections"] = Value::Array(sections);
    // `<` never appears in the JSON as itself, so no text can end the script.
    let data = data.to_string().replace('<', "\\u003c");
    Ok(fill(REVIEW_PAGE, &[("DATA", &data)]))
}

#[cfg(test)]
#[path = "design_render_tests.rs"]
mod tests;
