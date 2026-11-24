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
pub fn headings(
    index: &Value,
    first: &str,
    boards: &BTreeMap<&str, &[Value]>,
) -> Result<BTreeMap<String, Vec<Value>>> {
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
        .map(|(page, found)| {
            (
                page.to_string(),
                entries(found, boards.get(page).copied().unwrap_or(&[])),
            )
        })
        .collect())
}

/// <found> as entries in order of their y, a platform pair as one entry. Each
/// heading that has no sub-entries, and each sub-entry, has the names of the
/// page's <boards> (each a `{ "name", "y", ... }` object) in its row: those
/// from its y down to the next heading's y, the first row also holding the
/// boards above it. A board with no y is in no row.
fn entries(mut found: Vec<Heading>, boards: &[Value]) -> Vec<Value> {
    found.sort_by(|a, b| a.y.total_cmp(&b.y).then_with(|| a.text.cmp(b.text)));
    let row = |n: usize| -> Value {
        let from = if n == 0 {
            f64::NEG_INFINITY
        } else {
            found[n].y
        };
        let to = found.get(n + 1).map_or(f64::INFINITY, |next| next.y);
        boards
            .iter()
            .filter(|b| b["y"].as_f64().is_some_and(|y| y >= from && y < to))
            .map(|b| b["name"].clone())
            .collect()
    };
    let leaf = |n: usize, name: &str| json!({ "name": name, "y": found[n].y, "boards": row(n) });
    let mut out = Vec::new();
    let mut paired: Vec<&str> = Vec::new();
    for (n, heading) in found.iter().enumerate() {
        let Some((stem, _)) = split(heading.text) else {
            out.push(leaf(n, heading.text));
            continue;
        };
        if paired.contains(&stem) {
            continue;
        }
        let pair = PLATFORMS.map(|platform| only(&found, stem, platform));
        let [Some(a), Some(b)] = pair else {
            out.push(leaf(n, heading.text));
            continue;
        };
        paired.push(stem);
        // The pair's sub-entries stand in order of their y, which is the
        // order of `found`.
        let (first, second) = if a < b { (a, b) } else { (b, a) };
        let sub = |at: usize| leaf(at, split(found[at].text).expect("a platform heading").1);
        out.push(json!({ "name": stem, "subs": [sub(first), sub(second)] }));
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

/// The place in <found> of the one heading that is <stem> for <platform>;
/// none when there is none or more than one.
fn only(found: &[Heading], stem: &str, platform: &str) -> Option<usize> {
    let mut each = found
        .iter()
        .enumerate()
        .filter(|(_, h)| split(h.text) == Some((stem, platform)))
        .map(|(n, _)| n);
    match (each.next(), each.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

#[cfg(test)]
#[path = "designs_headings_tests.rs"]
mod tests;
