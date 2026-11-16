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

#[test]
fn a_checkout_of_a_revision_holds_its_files_and_is_removed_when_dropped() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = Source {
        repo: repo.to_path_buf(),
        rev: "HEAD".to_string(),
        in_place: false,
    };
    let checkout = source.checkout().unwrap();
    let path = checkout.path.clone();
    assert!(path.join("Cargo.toml").is_file());
    drop(checkout);
    assert!(!path.exists());
    let here = Source {
        in_place: true,
        ..source
    };
    let in_place = here.checkout().unwrap();
    assert_eq!(in_place.path, repo);
    drop(in_place);
    assert!(repo.exists(), "a checkout read in place is never removed");
}

#[test]
fn commit_messages_become_one_added_file_with_quotes_to_check_against() {
    let (file, quotes) = commit_message_file("First line\n\nBody line\n").unwrap();
    assert_eq!(file.path, COMMIT_MESSAGE);
    assert!(
        file.text
            .contains("@@ -0,0 +1,3 @@\n+First line\n+\n+Body line\n")
    );
    assert_eq!(quotes.lines, vec!["First line", "", "Body line"]);
    assert!(commit_message_file("  \n").is_none());
}

#[test]
fn a_renamed_file_changes_its_old_and_its_new_path() {
    let diff = "diff --git a/src/a.rs b/tool/a.rs\nsimilarity index 90%\nrename from src/a.rs\nrename to tool/a.rs\n--- a/src/a.rs\n+++ b/tool/a.rs\n@@ -1 +1 @@\n-rename from x\n+y\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n+rename from z\n";
    let files = split(diff);
    assert_eq!(files[0].paths(), ["tool/a.rs", "src/a.rs"]);
    assert_eq!(files[1].paths(), ["b.rs"], "a hunk line names no rename");
    assert_eq!(paths(diff), ["tool/a.rs", "src/a.rs", "b.rs"]);
}
