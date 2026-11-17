use super::*;

const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,4 @@\n fn keep() {}\n-fn old() {}\n+fn new() {\n+    let x = rows.iter().filter(|r| r.ok).count();\n+}\ndiff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-diff --git a/x b/x\n+text\n";

#[test]
fn a_clause_is_found_through_whitespace_and_markdown_marks() {
    let rule = "**Never mask errors.**\nA default\nhides   the failure.";
    assert!(clause_in("A default hides the failure.", rule));
    assert!(clause_in("Never mask errors", rule));
    assert!(!clause_in("A default shows the failure", rule));
    assert!(!clause_in("   ", rule));
}

#[test]
fn the_diff_is_split_by_file_and_each_line_loses_its_mark() {
    let files = parse_diff(DIFF);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "b.txt"]);
    assert_eq!(files[0].lines[0], "fn keep() {}");
    // A removed line that looks like a diff header stays a line.
    assert_eq!(files[1].lines, ["diff --git a/x b/x", "text"]);
}

#[test]
fn code_is_checked_against_the_named_file_across_lines() {
    let files = parse_diff(DIFF);
    assert!(code_in(&files, "src/a.rs", "rows.iter().filter(|r| r.ok).count()").is_ok());
    assert!(code_in(&files, "a.rs", "fn new() {\n    let x").is_ok());
    assert!(code_in(&files, "src/a.rs", "text").is_err());
    assert!(code_in(&files, "b.txt", "text").is_ok());
    assert!(
        code_in(&files, "missing.rs", "text")
            .unwrap_err()
            .contains("no file")
    );
    assert!(code_in(&files, "src/a.rs", "").is_err());
}

#[test]
fn the_changed_lines_are_the_lines_the_hunks_add_and_remove() {
    assert_eq!(changed_lines(DIFF), 6);
    assert_eq!(changed_lines(""), 0);
}

#[test]
fn a_diff_header_names_the_new_path() {
    assert_eq!(new_path("diff --git a/x.rs b/y.rs\n"), Some("y.rs"));
    assert_eq!(new_path("+let x = 1;"), None);
}

#[test]
fn each_language_marks_its_comment_lines() {
    for (path, line) in [
        ("a.rs", "    /// Sends the cover."),
        ("a.rs", "// a note"),
        ("a.swift", "  /// Sends the cover."),
        ("a.kt", " * Sends the cover."),
        ("a.kt", " */"),
        ("a.cs", "/// <summary>Sends</summary>"),
        ("a.py", "    # a note"),
        ("scripts/a.sh", "# a note"),
        ("Cargo.toml", "# a note"),
        ("a.sql", "-- a note"),
    ] {
        assert!(is_comment_line(path, line), "{path}: {line}");
    }
    for (path, line) in [
        ("a.rs", "    *count += 1;"),
        ("a.rs", "    * factor"),
        ("a.rs", "let url = \"http://x\"; // a note"),
        ("a.py", "x = 1  # a note"),
        ("a.sql", "SELECT 1;"),
        ("README.md", "# A heading"),
        ("a.md", "* a list item"),
    ] {
        assert!(!is_comment_line(path, line), "{path}: {line}");
    }
}

const COMMENTED: &str = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1,6 @@\n+/// Sends the cover\n+/// to the store.\n+fn send() -> &'static str {\n+    // the cover\n+    \"the cover\"\n+}\n";

#[test]
fn a_quote_is_on_comment_lines_only_when_every_place_it_sits_is_a_comment() {
    let files = parse_diff(COMMENTED);
    assert!(only_on_comment_lines(&files, "a.rs", "Sends the cover"));
    assert!(only_on_comment_lines(
        &files,
        "a.rs",
        "Sends the cover\n/// to the store."
    ));
    assert!(only_on_comment_lines(&files, "a.rs", "/// to the store."));
    // "the cover" sits on a comment line and in a string.
    assert!(!only_on_comment_lines(&files, "a.rs", "the cover"));
    assert!(!only_on_comment_lines(&files, "a.rs", "fn send()"));
    assert!(!only_on_comment_lines(&files, "a.rs", "not in the diff"));
    assert!(!only_on_comment_lines(&files, "b.rs", "Sends the cover"));
}
