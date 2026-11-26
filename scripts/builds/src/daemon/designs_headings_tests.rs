use super::*;
use serde_json::json;

/// <found> as the JSON the page reads.
fn shown(found: BTreeMap<String, Vec<Entry>>) -> BTreeMap<String, Vec<Value>> {
    found
        .into_iter()
        .map(|(page, entries)| {
            let entries = entries
                .iter()
                .map(|entry| serde_json::to_value(entry).unwrap())
                .collect();
            (page, entries)
        })
        .collect()
}

fn note(page: &str, y: i64, text: &str) -> Value {
    json!({ "page": page, "x": 0, "y": y, "kind": "title1", "w": 240, "text": text })
}

#[test]
fn a_page_gets_the_notes_without_a_size_in_order_of_their_y() {
    let index = json!({ "notes": {
        "b": note("two", 300, "Second"),
        "a": note("two", 0, "First"),
        "words": { "page": "two", "x": 0, "y": 100, "size": 24, "text": "What the code needs" },
        "other": note("one", 40, "Elsewhere"),
        "no-page": { "x": 0, "y": 10, "text": "On the first page" },
    } });
    let found = shown(headings(&index, "one", &BTreeMap::new()).unwrap());
    assert_eq!(
        found["two"],
        vec![
            json!({ "name": "First", "y": 0.0, "boards": [] }),
            json!({ "name": "Second", "y": 300.0, "boards": [] })
        ]
    );
    assert_eq!(
        found["one"],
        vec![
            json!({ "name": "On the first page", "y": 10.0, "boards": [] }),
            json!({ "name": "Elsewhere", "y": 40.0, "boards": [] }),
        ]
    );
}

#[test]
fn an_index_without_notes_gives_no_headings() {
    assert!(
        headings(&json!({ "boards": {} }), "one", &BTreeMap::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_heading_drawn_for_android_and_for_macos_is_one_entry_with_both_as_sub_entries() {
    let index = json!({ "notes": {
        "a": note("p", 0, "Albums → album → artist: Android"),
        "b": note("p", 100, "Albums → album → artist: macOS"),
        "c": note("p", 200, "Library states: macOS"),
        "d": note("p", 300, "Library states: Android"),
        "e": note("p", 400, "Switch library: Android"),
        "f": note("p", 500, "Several selected: macOS"),
        "g": note("p", 600, "Errors"),
    } });
    assert_eq!(
        shown(headings(&index, "p", &BTreeMap::new()).unwrap())["p"],
        vec![
            json!({ "name": "Albums → album → artist", "subs": [
                { "name": "Android", "y": 0.0, "boards": [] }, { "name": "macOS", "y": 100.0, "boards": [] }] }),
            json!({ "name": "Library states", "subs": [
                { "name": "macOS", "y": 200.0, "boards": [] }, { "name": "Android", "y": 300.0, "boards": [] }] }),
            json!({ "name": "Switch library: Android", "y": 400.0, "boards": [] }),
            json!({ "name": "Several selected: macOS", "y": 500.0, "boards": [] }),
            json!({ "name": "Errors", "y": 600.0, "boards": [] }),
        ]
    );
}

#[test]
fn a_platform_drawn_twice_under_one_name_pairs_with_nothing() {
    let index = json!({ "notes": {
        "a": note("p", 0, "Sync: Android"),
        "b": note("p", 100, "Sync: Android"),
        "c": note("p", 200, "Sync: macOS"),
    } });
    assert_eq!(
        shown(headings(&index, "p", &BTreeMap::new()).unwrap())["p"],
        vec![
            json!({ "name": "Sync: Android", "y": 0.0, "boards": [] }),
            json!({ "name": "Sync: Android", "y": 100.0, "boards": [] }),
            json!({ "name": "Sync: macOS", "y": 200.0, "boards": [] }),
        ]
    );
}

#[test]
fn a_heading_with_no_text_or_no_y_fails_and_names_the_note() {
    let error = headings(
        &json!({ "notes": { "n": { "y": 0 } } }),
        "p",
        &BTreeMap::new(),
    )
    .unwrap_err()
    .text();
    assert!(error.contains("note n has no text"), "{error}");
    let error = headings(
        &json!({ "notes": { "n": { "text": "Row" } } }),
        "p",
        &BTreeMap::new(),
    )
    .unwrap_err()
    .text();
    assert!(error.contains("note n (\"Row\") gives no y"), "{error}");
}

#[test]
fn each_row_gets_the_boards_from_its_y_down_to_the_next_heading_and_the_first_also_those_above() {
    let index = json!({ "notes": {
        "a": note("p", 0, "Rows: Android"),
        "b": note("p", 400, "Rows: macOS"),
        "c": note("p", 900, "Alone"),
    } });
    let drawn = [("a", -50.0), ("b", 10.0), ("c", 400.0), ("d", 950.0)].map(|(name, y)| {
        DesignsBoard::Placed {
            name: name.to_string(),
            title: None,
            x: 0.0,
            y,
            w: 100,
            h: 100,
        }
    });
    let nowhere = [DesignsBoard::Failed {
        name: "e".to_string(),
        title: None,
        error: "no place".to_string(),
    }];
    let boards: BTreeMap<&str, &[DesignsBoard]> =
        BTreeMap::from([("p", &drawn[..]), ("q", &nowhere[..])]);
    assert_eq!(
        shown(headings(&index, "p", &boards).unwrap())["p"],
        vec![
            json!({ "name": "Rows", "subs": [
                { "name": "Android", "y": 0.0, "boards": ["a", "b"] },
                { "name": "macOS", "y": 400.0, "boards": ["c"] }] }),
            json!({ "name": "Alone", "y": 900.0, "boards": ["d"] }),
        ]
    );
}
