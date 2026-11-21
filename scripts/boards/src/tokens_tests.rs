use super::*;

/// The families and font stacks are out of alphabetical order, so the test
/// can check that the stylesheet keeps the order of the file.
const TOKENS: &str = r##"{
  "name": "demo",
  "version": 1,
  "meta": { "source": "a test" },
  "color": {
    "themes": [{ "id": "dark", "name": "Dark" }, { "id": "light", "name": "Light" }],
    "tokens": [
      { "name": "background", "value": { "dark": "#111111", "light": "#EEEEEE" }, "usage": "behind everything" },
      { "name": "danger", "value": "#FF0000" }
    ]
  },
  "type": {
    "fonts": [],
    "families": { "sans": "Inter, sans-serif", "mono": "Menlo, monospace" },
    "groups": [
      {
        "name": "Text",
        "family": "sans",
        "styles": [
          { "name": "body", "fontSize": "14px", "fontWeight": 400, "lineHeight": "1.45" },
          { "name": "code", "family": "mono", "fontSize": "13px", "fontWeight": "500", "letterSpacing": "0.01em" }
        ]
      }
    ]
  },
  "spacing": { "tokens": [{ "name": "space-edge", "value": "16px" }] },
  "radius": { "tokens": [{ "name": "radius-card", "value": "12px" }, { "name": "radius-chip", "value": "999px" }] }
}"##;

#[test]
fn the_stylesheet_has_a_block_per_theme_then_the_other_tokens_then_a_class_per_text_style() {
    assert_eq!(
        tokens_css(TOKENS).unwrap(),
        r#"/* demo: builds design tokens-css writes this file from tokens.json */
:root, [data-theme="dark"] {
  --background: #111111;
  --danger: #FF0000;
}
[data-theme="light"] {
  --background: #EEEEEE;
  --danger: #FF0000;
}
:root {
  --space-edge: 16px;
  --radius-card: 12px;
  --radius-chip: 999px;
  --font-sans: Inter, sans-serif;
  --font-mono: Menlo, monospace;
}
.body { font-family: var(--font-sans); font-size: 14px; font-weight: 400; line-height: 1.45; }
.code { font-family: var(--font-mono); font-size: 13px; font-weight: 500; letter-spacing: 0.01em; }
"#
    );
}

#[test]
fn tokens_that_cannot_make_a_stylesheet_say_why() {
    let no_light = TOKENS.replace(
        r##""dark": "#111111", "light": "#EEEEEE""##,
        r##""dark": "#111111""##,
    );
    assert_eq!(
        tokens_css(&no_light).unwrap_err().text(),
        "color background has no value for theme light"
    );
    let no_tokens = TOKENS.replace(r#""spacing": { "tokens""#, r#""spacing": { "list""#);
    let error = tokens_css(&no_tokens).unwrap_err().text();
    assert!(
        error.starts_with("reading the tokens of family spacing: missing field `tokens`"),
        "{error}"
    );
    let error = tokens_css("not json").unwrap_err().text();
    assert!(error.starts_with("reading the tokens: "), "{error}");
}

#[test]
fn the_specimen_gives_each_style_with_its_font_stack() {
    let specimen = specimen(TOKENS).unwrap();
    assert_eq!(
        specimen.styles,
        [
            TextSpecimen {
                name: "body".into(),
                css: "font-family: Inter, sans-serif; font-size: 14px; font-weight: 400; line-height: 1.45"
                    .into(),
                words: "body".into(),
            },
            TextSpecimen {
                name: "code".into(),
                css: "font-family: Menlo, monospace; font-size: 13px; font-weight: 500; letter-spacing: 0.01em"
                    .into(),
                words: "code".into(),
            },
        ]
    );
}

#[test]
fn the_color_names_are_the_themes_and_each_color_in_the_file_order() {
    let (themes, names) = color_names(TOKENS).unwrap();
    assert_eq!(
        themes,
        [
            ThemeName {
                id: "dark".into(),
                name: "Dark".into()
            },
            ThemeName {
                id: "light".into(),
                name: "Light".into()
            },
        ]
    );
    assert_eq!(names, ["background", "danger"]);
    let no_theme = TOKENS.replace(
        r#"[{ "id": "dark", "name": "Dark" }, { "id": "light", "name": "Light" }]"#,
        "[]",
    );
    assert_eq!(
        color_names(&no_theme).unwrap_err().text(),
        "the tokens name no color theme"
    );
}

#[test]
fn a_style_shows_its_sample_else_its_usage_and_a_family_with_no_stack_fails() {
    let with_words = TOKENS
        .replace(
            r#""lineHeight": "1.45" }"#,
            r#""lineHeight": "1.45", "usage": "Labels", "sample": "Low Tide Hours" }"#,
        )
        .replace(
            r#""letterSpacing": "0.01em" }"#,
            r#""letterSpacing": "0.01em", "usage": "Codes and paths" }"#,
        );
    let words: Vec<String> = specimen(&with_words)
        .unwrap()
        .styles
        .into_iter()
        .map(|style| style.words)
        .collect();
    assert_eq!(words, ["Low Tide Hours", "Codes and paths"]);
    let error = specimen(&TOKENS.replace(r#""family": "mono""#, r#""family": "serif""#))
        .unwrap_err()
        .text();
    assert!(
        error.contains("text style code is in the family serif"),
        "{error}"
    );
}
