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
    let files = parse_diff(DIFF).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "b.txt"]);
    let line = |text: &str, kind| DiffLine {
        text: text.to_string(),
        kind,
    };
    assert_eq!(
        files[0].lines[0],
        line(
            "fn keep() {}",
            LineKind::Context {
                before: 1,
                after: 1
            }
        )
    );
    assert_eq!(files[0].lines[1], line("fn old() {}", LineKind::Removed(2)));
    // A removed line that looks like a diff header stays a line.
    assert_eq!(
        files[1].lines,
        [
            line("diff --git a/x b/x", LineKind::Removed(1)),
            line("text", LineKind::Added(1))
        ]
    );
}

#[test]
fn code_is_checked_against_the_named_file_across_lines() {
    let files = parse_diff(DIFF).unwrap();
    assert!(
        code_site(
            &files,
            "src/a.rs",
            3,
            "rows.iter().filter(|r| r.ok).count()"
        )
        .is_ok()
    );
    assert!(code_site(&files, "a.rs", 2, "fn new() {\n    let x").is_ok());
    assert!(code_site(&files, "src/a.rs", 1, "text").is_err());
    assert!(code_site(&files, "b.txt", 1, "text").is_ok());
    assert!(
        code_site(&files, "missing.rs", 1, "text")
            .unwrap_err()
            .contains("no file")
    );
    assert!(code_site(&files, "src/a.rs", 1, "").is_err());
    assert!(
        code_site(&files, "src/a.rs", 2, "fn old() {}").is_ok(),
        "a removed line"
    );
}

#[test]
fn code_only_on_the_lines_around_the_changes_is_refused() {
    let files = parse_diff(DIFF).unwrap();
    let refused = code_site(&files, "src/a.rs", 1, "fn keep() {}").unwrap_err();
    assert!(refused.contains("around the diff's changes"), "{refused}");
    assert!(code_site(&files, "src/a.rs", 1, "fn keep() {}\nfn old() {}").is_ok());
}

#[test]
fn the_reported_removal_is_distinct_from_the_same_quote_added_elsewhere() {
    let files =
        parse_diff("diff --git a/a.rs b/a.rs\n@@ -1,2 +1,2 @@\n-repeat\n keep\n+repeat\n").unwrap();
    assert_eq!(
        code_site(&files, "a.rs", 1, "repeat"),
        Ok(QuoteSite {
            file: "a.rs",
            source: FilePosition {
                path: "a.rs",
                position: Position {
                    side: Side::Before,
                    line: 1
                }
            }
        })
    );
    assert_eq!(
        code_site(&files, "a.rs", 2, "repeat"),
        Ok(QuoteSite {
            file: "a.rs",
            source: FilePosition {
                path: "a.rs",
                position: Position {
                    side: Side::After,
                    line: 2
                }
            }
        })
    );
}

#[test]
fn a_reported_context_line_cannot_name_a_different_added_line() {
    let files = parse_diff("diff --git a/a.rs b/a.rs\n@@ -1 +1,2 @@\n repeat\n+repeat\n").unwrap();
    let error = code_site(&files, "a.rs", 1, "repeat").unwrap_err();
    assert!(error.contains("around the diff's changes"), "{error}");
}

#[test]
fn a_quote_at_the_same_number_on_both_changed_sides_is_ambiguous() {
    let files = parse_diff(
        "diff --git a/a.rs b/a.rs\n@@ -1,5 +1,5 @@\n one\n-older\n-repeat\n keep\n+repeat\n+new\n end\n",
    ).unwrap();
    let error = code_site(&files, "a.rs", 3, "repeat").unwrap_err();
    assert!(error.contains("both sides"), "{error}");
}

#[test]
fn a_quote_cannot_join_a_removed_line_to_an_added_line() {
    let files = parse_diff("diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-before\n+after\n").unwrap();
    let error = code_site(&files, "a.rs", 1, "before after").unwrap_err();
    assert!(error.contains("not in the diff"), "{error}");
}

#[test]
fn a_line_that_identifies_neither_side_cannot_select_an_addition() {
    let files =
        parse_diff("diff --git a/a.rs b/a.rs\n@@ -1,2 +1,2 @@\n-repeat\n keep\n+repeat\n").unwrap();
    let error = code_site(&files, "a.rs", 99, "repeat").unwrap_err();
    assert!(error.contains("more than once"), "{error}");
}

#[test]
fn a_reported_old_context_line_cannot_name_a_different_removal() {
    let files = parse_diff(
        "diff --git a/a.rs b/a.rs\n@@ -1,4 +1,2 @@\n-prelude\n repeat\n-repeat\n other\n",
    )
    .unwrap();
    let error = code_site(&files, "a.rs", 2, "repeat").unwrap_err();
    assert!(error.contains("around the diff's changes"), "{error}");
}

#[test]
fn a_quote_removed_during_a_rename_keeps_the_old_path() {
    let files = parse_diff(concat!(
        "diff --git a/old.rs b/new.rs\n",
        "rename from old.rs\nrename to new.rs\n",
        "@@ -1,2 +1 @@\n-removed\n keep\n",
    ))
    .unwrap();
    let site = code_site(&files, "new.rs", 1, "removed").unwrap();
    assert_eq!(site.file, "new.rs");
    assert_eq!(site.source.path, "old.rs");
}

#[test]
fn a_report_for_an_exact_path_never_borrows_a_quote_from_another_file() {
    let files = parse_diff(concat!(
        "diff --git a/a.rs b/a.rs\n@@ -0,0 +1 @@\n+other\n",
        "diff --git a/src/a.rs b/src/a.rs\n@@ -0,0 +1 @@\n+repeat\n",
    ))
    .unwrap();
    let error = code_site(&files, "a.rs", 1, "repeat").unwrap_err();
    assert!(error.contains("not in the diff of a.rs"), "{error}");
    assert_eq!(
        code_site(&files, "src/a.rs", 1, "repeat"),
        Ok(QuoteSite {
            file: "src/a.rs",
            source: FilePosition {
                path: "src/a.rs",
                position: Position {
                    side: Side::After,
                    line: 1
                }
            }
        })
    );
}

#[test]
fn the_changed_lines_are_the_lines_the_hunks_add_and_remove() {
    assert_eq!(changed_lines(DIFF), 6);
    assert_eq!(changed_lines(""), 0);
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
    let files = parse_diff(COMMENTED).unwrap();
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

fn diff_of_lines(lines: usize) -> String {
    let mut diff = format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -0,0 +1,{lines} @@\n");
    diff.push_str(&"+x\n".repeat(lines));
    diff.push_str("-gone\n");
    diff
}

#[test]
fn added_lines_counts_the_lines_a_diff_adds_and_not_those_it_removes() {
    assert_eq!(added_lines(&diff_of_lines(3)), 3);
}

#[test]
fn at_most_three_reviewers_run_at_once() {
    use std::sync::{Barrier, Mutex};
    // Each call waits until three calls wait together, so three workers run
    // at once, and the set of threads that ran a call shows no fourth.
    let together = Barrier::new(REVIEWERS_AT_ONCE);
    let threads = Mutex::new(std::collections::BTreeSet::new());
    let items: Vec<usize> = (0..12).collect();
    let doubled = reviewers_map(&items, |n| {
        together.wait();
        threads
            .lock()
            .unwrap()
            .insert(format!("{:?}", std::thread::current().id()));
        n * 2
    });
    assert_eq!(doubled, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    assert_eq!(REVIEWERS_AT_ONCE, 3);
    assert_eq!(threads.lock().unwrap().len(), REVIEWERS_AT_ONCE);
}
