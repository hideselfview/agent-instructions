//! The row headings of a canvas page, which the Designs page lists as its
//! contents column. A heading is a note of the repo's canvas index
//! (`notes`) that gives no `size`; the notes with a size are words about the
//! boards. A heading that the index draws once for Android and once for macOS
//! ("Library states: Android", "Library states: macOS") is one entry with
//! the two platforms as its sub-entries.

use std::collections::BTreeMap;

use boards::CANVAS;
use serde_json::{Value, json};

use crate::design_canvas::page_of;
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
/// drawn once for each platform, a `{ "name", "y", "subs" }` object whose
/// `subs` are the two platforms' `{ "name", "y" }` objects, and whose `y` is
/// the first one's. <first> is the page of a note that names none. Fails
/// when a note that is a heading has no text or no y.
pub fn headings(index: &Value, first: &str) -> Result<BTreeMap<String, Vec<Value>>> {
    let mut by_page: BTreeMap<&str, Vec<Heading>> = BTreeMap::new();
    let notes = match index.get("notes") {
        None => return Ok(BTreeMap::new()),
        Some(Value::Object(notes)) => notes,
        Some(_) => fail!("{CANVAS}: `notes` is not an object of notes"),
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
            "y": subs[0].1,
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

#[cfg(test)]
#[path = "designs_headings_tests.rs"]
mod tests;
