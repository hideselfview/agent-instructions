//! One segment of a board or shot name, the text between its dots.

/// Whether <text> is a segment: lower-case words, of letters and digits,
/// joined by `-`.
pub fn is_segment(text: &str) -> bool {
    text.split('-').all(|word| {
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}
