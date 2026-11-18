//! The boards and the canvas index: a board is a design file,
//! `design/mocks/<name>.dc.html`, and `design/mocks/canvas.json` places each
//! board on the canvas, gives its size, and names the design system the
//! boards load.

use serde_json::Value;

use crate::{Result, fail};

pub const MOCKS: &str = "design/mocks";
/// The file that places the boards on the canvas and gives each its size.
pub const CANVAS: &str = "design/mocks/canvas.json";
pub const BOARD_SUFFIX: &str = ".dc.html";

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

/// The path in the repo of the board named <name>.
pub fn board_path(name: &str) -> String {
    format!("{MOCKS}/{name}{BOARD_SUFFIX}")
}

/// The names of the boards that board markup <body> mounts (`dc-import`).
pub fn imports(body: &str) -> Vec<String> {
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

/// The width and height that <canvas>, a parsed `canvas.json`, gives board
/// file <file>.
pub fn canvas_size(canvas: &Value, file: &str) -> Result<(u64, u64)> {
    let board = &canvas["boards"][file];
    match (board["w"].as_u64(), board["h"].as_u64()) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Ok((w, h)),
        _ => fail!("{CANVAS} gives no width and height for {file}"),
    }
}

/// The namespace of the one design system <canvas>, a parsed `canvas.json`,
/// names: the folder the boards load it from, `ds/<namespace>/`.
pub fn design_namespace(canvas: &Value) -> Result<String> {
    let systems = canvas["designSystems"].as_array().map(Vec::as_slice);
    let [system] = systems.unwrap_or_default() else {
        fail!("{CANVAS} names no design system, or more than one; the boards load one");
    };
    let namespace = system["namespace"].as_str().unwrap_or("");
    let fine = namespace
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && namespace.len() <= 64
        && namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'));
    if !fine {
        fail!(
            "{CANVAS}'s design system has the namespace {namespace:?}, which is no folder name on a canvas"
        );
    }
    Ok(namespace.to_string())
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod tests;
