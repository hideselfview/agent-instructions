use super::*;

const DIFF: &str = "From abc Mon Sep 17 00:00:00 2001\nSubject: x\n\ndiff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@\n x\n+y\n z\ndiff --git a/dir/b.rs b/dir/b.rs\n--- a/dir/b.rs\n+++ b/dir/b.rs\n@@ -5 +5,2 @@\n+q\n";

#[test]
fn split_cuts_at_each_file_and_drops_the_mail_header() {
    let files = split(DIFF);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "a.rs");
    assert_eq!(files[1].path, "dir/b.rs");
    assert!(files[0].text.starts_with("diff --git a/a.rs"));
    assert!(!files[0].text.contains("dir/b.rs"));
}

#[test]
fn hunks_read_new_ranges_with_and_without_a_count() {
    assert_eq!(hunks(&split(DIFF)[0].text), vec![(1, 3)]);
    assert_eq!(hunks(&split(DIFF)[1].text), vec![(5, 2)]);
}

#[test]
fn no_source_gives_no_context() {
    assert_eq!(context(None, "a.rs", DIFF), "");
}
