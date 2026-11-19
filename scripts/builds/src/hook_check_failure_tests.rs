use super::*;

#[test]
fn a_failed_check_names_the_files_it_was_handed_that_its_output_names() {
    let failure = CheckFailure::of(
        "Diff in /w/side-2/src/b.rs at line 3:\n-a\n+b\n",
        &["src/c.rs".to_string(), "src/b.rs".to_string()],
    );
    assert_eq!(failure.files, vec!["src/b.rs".to_string()]);
    assert_eq!(
        failure.excerpt.as_deref(),
        Some("Diff in /w/side-2/src/b.rs at line 3:\n-a\n+b")
    );
    assert_eq!(CheckFailure::of("", &[]), CheckFailure::default());
}
