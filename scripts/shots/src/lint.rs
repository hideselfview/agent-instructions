//! Shots declared with no board to compare against. A screen shot's board is
//! `design/mocks/<name>.dc.html`; a component shot's board is the named row of
//! `design/system/components/<Component>/preview.html`, or the one state
//! `default` when the preview names no row. The lint reads a preview's rows
//! without a browser: the first string argument of each call of the
//! functions the app's `shots-preview-calls` line names.

use crate::declared::Declared;
use crate::names::{Name, Previews, state_id};
use crate::record::{FILE, Record};
use regex::Regex;
use std::path::Path;

const BOARDS: &str = boards::MOCKS;

/// The ids of the named rows of a preview page: the first argument of each
/// call of one of <calls>, such as `N('…', …)`.
fn states(html: &str, calls: &[String]) -> Vec<String> {
    if calls.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = calls.iter().map(|c| regex::escape(c)).collect();
    let row = Regex::new(&format!(r"\b(?:{})\('((?:[^'\\]|\\.)*)'", names.join("|"))).unwrap();
    row.captures_iter(html)
        .map(|c| state_id(&c[1].replace("\\'", "'")))
        .collect()
}

/// The state ids of a preview page's rows, each once, in the page's order;
/// `default` alone for a page with no named rows, which draws one state.
pub(crate) fn rows(html: &str, calls: &[String]) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for state in states(html, calls) {
        if !rows.contains(&state) {
            rows.push(state);
        }
    }
    if rows.is_empty() {
        rows.push("default".to_string());
    }
    rows
}

/// A shot or a record entry with no board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) name: String,
    /// True for a shot a harness declares, false for an entry of the record.
    pub(crate) declared: bool,
    pub(crate) sentence: String,
}

/// One problem for each declared shot and each entry of the record that has
/// no board. <calls> are the functions whose calls name a preview's rows, and
/// <previews> the components a component shot can draw.
pub(crate) fn problems(
    root: &Path,
    calls: &[String],
    previews: &Previews,
    declared: &[Declared],
    record: &Record,
) -> Vec<Problem> {
    let shots = declared.iter().filter_map(|d| {
        no_board(root, calls, previews, &d.name).map(|why| Problem {
            name: d.name.to_string(),
            declared: true,
            sentence: format!("shot '{}' has no board: {why}", d.name),
        })
    });
    let entries = record.names().filter_map(|text| {
        let why = match Name::parse(text) {
            Ok(name) => no_board(root, calls, previews, &name)?,
            Err(e) => e.to_string(),
        };
        Some(Problem {
            name: text.clone(),
            declared: false,
            sentence: format!("{FILE} has an entry '{text}' with no board: {why}"),
        })
    });
    shots.chain(entries).collect()
}

/// Why `name` has no board, when it has none.
fn no_board(root: &Path, calls: &[String], previews: &Previews, name: &Name) -> Option<String> {
    match name {
        Name::Screen(_) => {
            let board = Path::new(BOARDS).join(format!("{name}.dc.html"));
            match std::fs::metadata(root.join(&board)) {
                Ok(found) if found.is_file() => None,
                Ok(_) => Some(format!("{} is not a file", board.display())),
                Err(error) => Some(unreadable(&board, &error)),
            }
        }
        Name::Component(c) => {
            let component = c.component(previews);
            let preview = std::path::PathBuf::from(boards::preview::preview_path(&component));
            let html = match std::fs::read_to_string(root.join(&preview)) {
                Ok(html) => html,
                Err(error) => return Some(unreadable(&preview, &error)),
            };
            let states = rows(&html, calls);
            (!states.iter().any(|s| s == c.state()))
                .then(|| format!("{component} has the states {}", states.join(", ")))
        }
    }
}

/// Why <path> gave no board: it is missing, or reading it failed.
fn unreadable(path: &Path, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        format!("{} is missing", path.display())
    } else {
        format!("cannot read {}: {error}", path.display())
    }
}

#[cfg(test)]
#[path = "lint_tests.rs"]
mod tests;
