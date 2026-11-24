//! The row headings of a canvas page, which the Designs page lists as its
//! contents column. A heading is a note of the repo's canvas index
//! (`notes`) that gives no `size`; the notes with a size are words about the
//! boards. A heading that the index draws once for Android and once for macOS
//! ("Library states: Android", "Library states: macOS") is one entry with
//! the two platforms as its sub-entries.

use std::collections::BTreeMap;

use boards::CANVAS;
use serde_json::{Value, json};

use crate::design_canvas::{notes_of, page_of};
use crate::fail;
use crate::fail::Result;

/// The platforms that a heading names after its colon, in the order a
/// pair's sub-entries stand.
const PLATFORMS: [&str; 2] = ["Android", "macOS"];

/// One heading of the index: its text and the y of its row.
struct Heading<'a> {
    text: &'a str,
    y: f64,
}

/// The entries of each page of <index>, the repo's `CANVAS`, by page id:
/// each a `{ "name", "y" }` object in order of its y, or, for a heading
/// drawn once for each platform, a `{ "name", "subs" }` object whose `subs`
/// are the two platforms' `{ "name", "y" }` objects in order of their y.
/// <first> is the page of a note that names none. Fails
/// when a note that is a heading has no text or no y.
pub fn headings(index: &Value, first: &str) -> Result<BTreeMap<String, Vec<Value>>> {
    let mut by_page: BTreeMap<&str, Vec<Heading>> = BTreeMap::new();
    let Some(notes) = notes_of(index)? else {
        return Ok(BTreeMap::new());
    };
    for (id, note) in notes {
        if note.get("size").is_some() {
            continue;
        }
        let Some(text) = note["text"].as_str() else {
            fail!("{CANVAS}: note {id} has no text");
        };
        let Some(y) = note["y"].as_f64() else {
            fail!("{CANVAS}: note {id} ({text:?}) gives no y");
        };
        by_page
            .entry(page_of("note", id, note, first)?)
            .or_default()
            .push(Heading {
                text: text.trim(),
                y,
            });
    }
    Ok(by_page
        .into_iter()
        .map(|(page, found)| (page.to_string(), entries(found)))
        .collect())
}

/// <found> as entries in order of their y, a platform pair as one entry.
fn entries(mut found: Vec<Heading>) -> Vec<Value> {
    found.sort_by(|a, b| a.y.total_cmp(&b.y).then_with(|| a.text.cmp(b.text)));
    let mut out = Vec::new();
    let mut paired: Vec<&str> = Vec::new();
    for heading in &found {
        let Some((stem, _)) = split(heading.text) else {
            out.push(json!({ "name": heading.text, "y": heading.y }));
            continue;
        };
        if paired.contains(&stem) {
            continue;
        }
        let pair: Vec<Option<&Heading>> = PLATFORMS
            .iter()
            .map(|platform| only(&found, stem, platform))
            .collect();
        let [Some(_), Some(_)] = pair[..] else {
            out.push(json!({ "name": heading.text, "y": heading.y }));
            continue;
        };
        paired.push(stem);
        let mut subs: Vec<(&str, f64)> = pair
            .iter()
            .zip(PLATFORMS)
            .map(|(h, platform)| (platform, h.expect("both platforms are there").y))
            .collect();
        subs.sort_by(|a, b| a.1.total_cmp(&b.1));
        out.push(json!({
            "name": stem,
            "subs": subs.iter().map(|(name, y)| json!({ "name": name, "y": y })).collect::<Vec<_>>(),
        }));
    }
    out
}

/// <text> as the words before its platform and the platform, when it ends
/// in ": " and one of the `PLATFORMS`.
fn split(text: &str) -> Option<(&str, &str)> {
    PLATFORMS.iter().find_map(|platform| {
        text.strip_suffix(platform)
            .and_then(|rest| rest.strip_suffix(": "))
            .map(|stem| (stem, *platform))
    })
}

/// The one heading in <found> that is <stem> for <platform>; none when
/// there is none or more than one.
fn only<'a, 'b>(found: &'b [Heading<'a>], stem: &str, platform: &str) -> Option<&'b Heading<'a>> {
    let mut each = found
        .iter()
        .filter(|h| split(h.text) == Some((stem, platform)));
    match (each.next(), each.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// <entries>, a page's entries from `headings`, with each heading that has
/// no sub-entries, and each sub-entry, given the names of the page's <boards>
/// (each a `{ "name", "y", ... }` object) in its row: those from its y down
/// to the next heading's y, the first row also holding the boards above it.
/// A board with no y is in no row.
pub fn with_rows(entries: Vec<Value>, boards: &[Value]) -> Vec<Value> {
    let mut ys: Vec<f64> = entries
        .iter()
        .flat_map(|e| match e["subs"].as_array() {
            Some(subs) => subs.iter().map(|s| s["y"].as_f64()).collect(),
            None => vec![e["y"].as_f64()],
        })
        .flatten()
        .collect();
    ys.sort_by(f64::total_cmp);
    let boards_of = |y: f64| -> Vec<Value> {
        let n = ys.iter().position(|other| *other == y).unwrap_or(0);
        let from = if n == 0 { f64::NEG_INFINITY } else { y };
        let to = ys.get(n + 1).copied().unwrap_or(f64::INFINITY);
        boards
            .iter()
            .filter(|b| b["y"].as_f64().is_some_and(|by| by >= from && by < to))
            .map(|b| b["name"].clone())
            .collect()
    };
    let row = |mut leaf: Value| {
        leaf["boards"] = Value::Array(boards_of(leaf["y"].as_f64().unwrap_or(0.0)));
        leaf
    };
    entries
        .into_iter()
        .map(|mut e| match e["subs"].as_array().cloned() {
            Some(subs) => {
                e["subs"] = Value::Array(subs.into_iter().map(row).collect());
                e
            }
            None => row(e),
        })
        .collect()
}

#[cfg(test)]
#[path = "designs_headings_tests.rs"]
mod tests;
