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
