use super::*;

#[test]
fn question_mark_is_a_wildcard_or_plain_as_the_caller_says() {
    assert!(segment_matches("a?c", "abc", Question::Wildcard));
    assert!(!segment_matches("a?c", "ac", Question::Wildcard));
    assert!(!segment_matches("a?c", "abc", Question::Plain));
    assert!(segment_matches("a?c", "a?c", Question::Plain));
}

#[test]
fn a_double_star_segment_matches_any_number_of_segments() {
    for question in [Question::Wildcard, Question::Plain] {
        assert!(segments_match(
            &["src", "**", "*.rs"],
            &["src", "a.rs"],
            question
        ));
        assert!(segments_match(
            &["src", "**", "*.rs"],
            &["src", "x", "y", "a.rs"],
            question
        ));
        assert!(!segments_match(
            &["src", "**", "*.rs"],
            &["lib", "a.rs"],
            question
        ));
        assert!(!segments_match(
            &["src", "*.rs"],
            &["src", "x", "a.rs"],
            question
        ));
    }
}
