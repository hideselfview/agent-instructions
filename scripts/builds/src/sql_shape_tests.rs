use super::*;

#[test]
fn comments_and_spacing_leave_the_shape_of_the_statements() {
    assert_eq!(
        shape("CREATE TABLE t (\n    -- A note.\n    x   TEXT /* more */ NOT NULL\n) STRICT;\n"),
        "CREATE TABLE t ( x TEXT NOT NULL ) STRICT;"
    );
}

#[test]
fn a_quote_keeps_its_text_as_written() {
    assert_eq!(
        shape("CHECK (x <> '-- a  /* b */ it''s')  AND \"a--b\" AND [c  d] AND `e  f`"),
        "CHECK (x <> '-- a  /* b */ it''s') AND \"a--b\" AND [c  d] AND `e  f`"
    );
}

#[test]
fn an_unfinished_comment_or_quote_reads_to_the_end() {
    assert_eq!(shape("a /* b"), "a");
    assert_eq!(shape("a -- b"), "a");
    assert_eq!(shape("a 'b  c"), "a 'b  c");
}

#[test]
fn a_character_of_several_bytes_passes_through_whole() {
    assert_eq!(shape("a  'é  ü'  -- ö\n  ß"), "a 'é  ü' ß");
}
