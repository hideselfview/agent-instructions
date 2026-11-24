use super::*;

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
    let found = headings(&index, "one").unwrap();
    assert_eq!(
        found["two"],
        vec![
            json!({ "name": "First", "y": 0.0 }),
            json!({ "name": "Second", "y": 300.0 })
        ]
    );
    assert_eq!(
        found["one"],
        vec![
            json!({ "name": "On the first page", "y": 10.0 }),
            json!({ "name": "Elsewhere", "y": 40.0 }),
        ]
    );
}

#[test]
fn an_index_without_notes_gives_no_headings() {
    assert!(
        headings(&json!({ "boards": {} }), "one")
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
        headings(&index, "p").unwrap()["p"],
        vec![
            json!({ "name": "Albums → album → artist", "y": 0.0, "subs": [
                { "name": "Android", "y": 0.0 }, { "name": "macOS", "y": 100.0 }] }),
            json!({ "name": "Library states", "y": 200.0, "subs": [
                { "name": "macOS", "y": 200.0 }, { "name": "Android", "y": 300.0 }] }),
            json!({ "name": "Switch library: Android", "y": 400.0 }),
            json!({ "name": "Several selected: macOS", "y": 500.0 }),
            json!({ "name": "Errors", "y": 600.0 }),
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
        headings(&index, "p").unwrap()["p"],
        vec![
            json!({ "name": "Sync: Android", "y": 0.0 }),
            json!({ "name": "Sync: Android", "y": 100.0 }),
            json!({ "name": "Sync: macOS", "y": 200.0 }),
        ]
    );
}

#[test]
fn a_heading_with_no_text_or_no_y_fails_and_names_the_note() {
    let error = headings(&json!({ "notes": { "n": { "y": 0 } } }), "p")
        .unwrap_err()
        .text();
    assert!(error.contains("note n has no text"), "{error}");
    let error = headings(&json!({ "notes": { "n": { "text": "Row" } } }), "p")
        .unwrap_err()
        .text();
    assert!(error.contains("note n (\"Row\") gives no y"), "{error}");
}
