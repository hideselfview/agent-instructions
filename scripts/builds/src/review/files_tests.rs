use super::*;

const DIFF: &str = "From abc Mon Sep 17 00:00:00 2001\nSubject: x\n\ndiff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@\n x\n+y\n z\ndiff --git a/dir/b.rs b/dir/b.rs\n--- a/dir/b.rs\n+++ b/dir/b.rs\n@@ -5 +5,2 @@\n+q\n";

#[test]
fn a_diff_header_names_the_new_path() {
    assert_eq!(
        new_path("diff --git a/x.rs b/y.rs\n").as_deref(),
        Some("y.rs")
    );
    assert_eq!(new_path("+let x = 1;"), None);
}

#[test]
fn a_diff_header_without_distinct_paths_cannot_name_a_file() {
    assert!(new_path("diff --git old.rs new.rs\n").is_none());
}

#[test]
fn a_path_containing_the_diff_separator_keeps_its_full_name() {
    let patch = concat!(
        "diff --git a/x b/f.rs b/x b/f.rs\n",
        "--- a/x b/f.rs\t\n",
        "+++ b/x b/f.rs\t\n",
        "@@ -1 +1 @@\n-old\n+new\n",
    );
    assert_eq!(paths(patch), ["x b/f.rs"]);
    let mode_change = "diff --git a/x b/f.rs b/x b/f.rs\nold mode 100644\nnew mode 100755\n";
    assert_eq!(paths(mode_change), ["x b/f.rs"]);
}

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
fn hunks_read_old_and_new_ranges_with_and_without_a_count() {
    let ranges = |diff: &str| {
        diff.lines()
            .filter_map(hunk_ranges)
            .map(|(old, new)| ((old.start, old.count), (new.start, new.count)))
            .collect::<Vec<_>>()
    };
    assert_eq!(ranges(DIFF), [((1, 2), (1, 3)), ((5, 1), (5, 2))]);
}

#[test]
fn no_source_gives_no_context() {
    assert_eq!(context(None, "a.rs", DIFF), "");
}

#[test]
fn mapping_a_line_reports_a_missing_review_commit() {
    let demo = crate::lists::item_fixtures::demo("review-missing-site-commit");
    let side = demo.add_side(1, "agent", "work");
    let head = demo.commit(&side, "a.rs");
    let missing = "missing-review-commit";
    let error = line_at(&side, missing, &head, "a.rs", 1)
        .unwrap_err()
        .text();
    assert!(error.contains(missing), "{error}");
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
    // Each checkout of the same commit gets its own folder, and dropping the
    // first checkout does not delete the second one's folder.
    let second = source.checkout().unwrap();
    assert_ne!(second.path, path);
    drop(checkout);
    assert!(!path.exists());
    assert!(second.path.join("Cargo.toml").is_file());
    drop(second);
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
    let texts: Vec<&str> = quotes.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, ["First line", "", "Body line"]);
    assert!(
        quotes
            .lines
            .iter()
            .all(crate::review::check::DiffLine::changed)
    );
    assert!(commit_message_file("  \n").is_none());
}

#[test]
fn a_renamed_file_changes_its_old_and_its_new_path() {
    let diff = "diff --git a/src/a.rs b/tool/a.rs\nsimilarity index 90%\nrename from src/a.rs\nrename to tool/a.rs\n--- a/src/a.rs\n+++ b/tool/a.rs\n@@ -1 +1 @@\n-rename from x\n+y\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n+rename from z\n";
    let files = split(diff);
    assert_eq!(files[0].paths(), ["tool/a.rs", "src/a.rs"]);
    assert_eq!(
        files[1].paths(),
        ["b.rs"],
        "a `rename from` line inside a hunk adds no path"
    );
    assert_eq!(paths(diff), ["tool/a.rs", "src/a.rs", "b.rs"]);
}

#[test]
fn a_rename_source_uses_the_filename_git_escaped_in_the_patch() {
    let diff = concat!(
        "diff --git \"a/a\\t\\303\\251.rs\" b/b.rs\n",
        "rename from \"a\\t\\303\\251.rs\"\n",
        "rename to b.rs\n",
    );
    assert_eq!(split(diff)[0].renamed_from.as_deref(), Some("a\té.rs"));
}

#[test]
fn a_rename_destination_uses_the_filename_git_escaped_in_the_patch() {
    let diff = concat!(
        "diff --git \"a/old\\t.rs\" \"b/new\\303\\251.rs\"\n",
        "rename from \"old\\t.rs\"\n",
        "rename to \"new\\303\\251.rs\"\n",
    );
    assert_eq!(split(diff)[0].paths(), ["newé.rs", "old\t.rs"]);
    assert_eq!(
        crate::review::cases::keep_files(diff, &["newé.rs".into()]).unwrap(),
        diff
    );
}
