use super::*;

#[test]
fn an_excerpt_starts_at_the_first_cause_else_holds_the_last_lines() {
    assert_eq!(
        Excerpt::of("Compiling a\nerror[E0308]: mismatched types\n --> src/a.rs:1:1\nmore"),
        Some("error[E0308]: mismatched types\n --> src/a.rs:1:1\nmore".to_string())
    );
    let long: String = (0..30).map(|n| format!("line {n}\n")).collect();
    let text = Excerpt::of(&long).unwrap();
    assert!(
        text.starts_with("line 10\n") && text.ends_with("line 29"),
        "{text}"
    );
    assert_eq!(Excerpt::of("\n  \n"), None);
}

#[test]
fn an_excerpt_holds_at_most_its_bytes_on_a_character_boundary() {
    let text = Excerpt::of(&format!("error: {}", "é".repeat(3000))).unwrap();
    assert!(text.len() <= 4000);
    assert!(text.starts_with("error: é"));
}
