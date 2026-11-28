use super::*;

fn only(path: &str, before: &str, after: &str) -> bool {
    let language = Language::of(path).expect("a language with a lexer");
    only_in_comments(language, Some(before), Some(after))
}

#[test]
fn a_changed_doc_comment_of_swift_or_kotlin_is_only_in_comments() {
    assert!(only(
        "Logo.swift",
        "/// The logo.\nstruct Logo {}\n",
        "/// The logo, drawn as the word bae.\nstruct Logo {}\n",
    ));
    assert!(only(
        "Logo.kt",
        "/** The logo. */\nclass Logo\n",
        "/**\n * The logo, drawn as the word bae.\n */\nclass Logo\n",
    ));
}

#[test]
fn adding_or_deleting_a_whole_comment_is_only_in_comments() {
    assert!(only(
        "a.swift",
        "let a = 1\n",
        "// Why a is one.\nlet a = 1\n"
    ));
    assert!(only("a.swift", "let a = 1 // one\n", "let a = 1\n"));
    assert!(only("a.cs", "int a = 1;\n", "int a = 1; /* one */\n"));
    assert!(only(
        "a.rs",
        "fn a() {}\n",
        "/// Says a.\n///\n/// Twice.\nfn a() {}\n"
    ));
}

#[test]
fn a_change_to_code_or_to_whitespace_outside_comments_is_code() {
    assert!(!only("a.swift", "let a = 1\n", "let a = 2\n"));
    assert!(!only("a.swift", "let a = 1\n", "let a  = 1\n"));
    assert!(!only("a.kt", "val a = 1\n", "val a = 1\n\n"));
    assert!(!only("a.swift", "let a = 1\n// note\n", "let a = 1\n\n"));
}

#[test]
fn a_comment_between_two_pieces_of_code_keeps_them_apart() {
    assert!(!only("a.cs", "int/**/a;\n", "inta;\n"));
    assert!(only("a.cs", "int/* x */a;\n", "int/* y */a;\n"));
}

#[test]
fn comment_marks_inside_strings_are_string_text() {
    assert!(!only(
        "a.swift",
        "let a = \"x // y\"\n",
        "let a = \"x // z\"\n"
    ));
    assert!(!only("a.kt", "val a = \"/* y\"\n", "val a = \"/* z\"\n"));
    assert!(!only(
        "a.swift",
        "let a = \"\"\"\n// y\n\"\"\"\n",
        "let a = \"\"\"\n// z\n\"\"\"\n"
    ));
    assert!(!only(
        "a.swift",
        "let a = #\"x \" // y\"#\n",
        "let a = #\"x \" // z\"#\n"
    ));
    assert!(!only(
        "a.kt",
        "val a = \"\"\"x \" // y\"\"\"\n",
        "val a = \"\"\"x \" // z\"\"\"\n"
    ));
    assert!(!only(
        "a.cs",
        "var a = @\"x \"\" // y\";\n",
        "var a = @\"x \"\" // z\";\n"
    ));
    assert!(!only(
        "a.cs",
        "var a = \"\"\"\n x \"\" // y\n \"\"\";\n",
        "var a = \"\"\"\n x \"\" // z\n \"\"\";\n"
    ));
    assert!(!only(
        "a.rs",
        "let a = r#\"x \" // y\"#;\n",
        "let a = r#\"x \" // z\"#;\n"
    ));
    assert!(!only(
        "a.rs",
        "let a = \"x \\\" // y\";\n",
        "let a = \"x \\\" // z\";\n"
    ));
}

#[test]
fn a_comment_after_a_string_is_a_comment() {
    assert!(only(
        "a.swift",
        "let a = \"x\" // y\n",
        "let a = \"x\" // z\n"
    ));
    assert!(only(
        "a.rs",
        "let a = r#\"x\"#; // y\n",
        "let a = r#\"x\"#; // z\n"
    ));
}

#[test]
fn interpolations_read_as_code_inside_their_strings() {
    assert!(!only(
        "a.swift",
        "let a = \"\\(f(\"//\")) y\"\n",
        "let a = \"\\(f(\"//\")) z\"\n"
    ));
    assert!(!only(
        "a.kt",
        "val a = \"${f(\"//\")} y\"\n",
        "val a = \"${f(\"//\")} z\"\n"
    ));
    assert!(!only(
        "a.cs",
        "var a = $\"{f(\"//\")} y\";\n",
        "var a = $\"{f(\"//\")} z\";\n"
    ));
    assert!(only(
        "a.kt",
        "val a = \"${f { it }}\" // y\n",
        "val a = \"${f { it }}\" // z\n"
    ));
}

#[test]
fn character_literals_and_lifetimes() {
    assert!(!only(
        "a.kt",
        "val q = '\"'; val a = \"// y\"\n",
        "val q = '\"'; val a = \"// z\"\n"
    ));
    assert!(!only(
        "a.rs",
        "let q = '\"'; let a = \"// y\";\n",
        "let q = '\"'; let a = \"// z\";\n"
    ));
    assert!(only(
        "a.rs",
        "fn a<'a>(x: &'a str) -> &'a str { x } // y\n",
        "fn a<'a>(x: &'a str) -> &'a str { x } // z\n"
    ));
}

#[test]
fn block_comments_nest_in_swift_kotlin_and_rust_and_not_in_csharp() {
    let before = "/* a /* b */ c */ let x = 1\n";
    let after = "/* a /* b */ d */ let x = 1\n";
    assert!(only("a.swift", before, after));
    assert!(only("a.kt", before, after));
    assert!(only("a.rs", before, after));
    assert!(!only("a.cs", before, after));
}

#[test]
fn a_file_added_or_removed_or_in_a_language_with_no_lexer_is_code() {
    assert!(!only_in_comments(Language::Swift, None, Some("// a\n")));
    assert!(!only_in_comments(Language::Swift, Some("// a\n"), None));
    assert_eq!(Language::of("a.m"), None);
}

#[test]
fn an_unchanged_file_is_no_change_only_in_comments() {
    assert!(!only("a.swift", "// a\nlet a = 1\n", "// a\nlet a = 1\n"));
}
