use super::*;

fn diff(hunks: &str) -> String {
    format!(
        "diff --git a/a.rs b/b.rs\nsimilarity index 90%\nrename from a.rs\nrename to b.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/b.rs\n@@ -1,4 +1,4 @@\n{hunks}"
    )
}

#[test]
fn a_file_renamed_with_no_change_is_a_rename() {
    let pure =
        "diff --git a/a.rs b/b.rs\nsimilarity index 100%\nrename from a.rs\nrename to b.rs\n";
    assert!(only_renames(pure));
}

#[test]
fn lines_that_differ_by_one_consistent_renaming_are_a_rename() {
    let renamed = diff(
        " fn keep() {}\n-let total = count(rows);\n+let sum = count(rows);\n fn tail() {}\n-    total += 1;\n+    sum += 1;\n",
    );
    assert!(only_renames(&renamed));
}

#[test]
fn a_changed_operator_literal_or_line_count_is_no_rename() {
    for hunks in [
        "-let a = 1;\n+let b = 2;\n",
        "-let a = b + c;\n+let a = b - c;\n",
        "-let a = 1;\n+let a = 1;\n+let c = 2;\n",
        "-let a = x;\n-let b = x;\n+let c = x;\n",
    ] {
        assert!(!only_renames(&diff(hunks)), "{hunks}");
    }
}

#[test]
fn one_old_name_renamed_two_ways_or_two_names_to_one_is_no_rename() {
    let two_ways = diff("-use a;\n+use b;\n-use a;\n+use c;\n");
    assert!(!only_renames(&two_ways));
    let merged = diff("-use a;\n+use c;\n-use b;\n+use c;\n");
    assert!(!only_renames(&merged));
}

#[test]
fn a_file_added_deleted_or_given_a_mode_is_no_rename() {
    for header in [
        "new file mode 100644",
        "deleted file mode 100644",
        "old mode 100644\nnew mode 100755",
    ] {
        let text = format!(
            "diff --git a/x b/x\n{header}\nindex 1..2\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+a\n"
        );
        assert!(!only_renames(&text), "{header}");
    }
}

#[test]
fn a_rename_in_a_real_repo_is_found_with_git_rename_detection() {
    let scratch = crate::testing::Scratch::new("mechanical-renames");
    let top = scratch.dir.join("top");
    crate::testing::repo(&top, "target 1G\n");
    let body = "fn keep() {}\nlet total = count(rows);\ntotal += 1;\nfn tail() {}\nfn more() {}\nfn last() {}\n";
    std::fs::write(top.join("a.rs"), body).unwrap();
    crate::testing::git(&top, &["add", "a.rs"]);
    crate::testing::git(&top, &["commit", "-q", "-m", "a"]);
    crate::testing::git(&top, &["mv", "a.rs", "b.rs"]);
    std::fs::write(top.join("b.rs"), body.replace("total", "sum")).unwrap();
    crate::testing::git(&top, &["add", "b.rs"]);
    crate::testing::git(&top, &["commit", "-q", "-m", "rename"]);
    let text = crate::review::run::diff_between(&top, "HEAD~1", "HEAD")
        .unwrap()
        .diff
        .text;
    assert!(text.contains("rename from a.rs"), "{text}");
    assert!(only_renames(&text), "{text}");
}
