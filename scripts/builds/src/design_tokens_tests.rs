use super::*;
use crate::testing::Scratch;

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
fn tokens_css_writes_the_stylesheet_beside_the_tokens() {
    let scratch = Scratch::new("tokens-css");
    let dir = scratch.dir.join("wt");
    assert!(
        write_tokens_css(&dir)
            .unwrap_err()
            .text()
            .starts_with(&format!("reading {}", dir.join(TOKENS_JSON).display()))
    );
    std::fs::create_dir_all(dir.join("design/system")).unwrap();
    std::fs::write(dir.join(TOKENS_JSON), TOKENS).unwrap();
    let said = write_tokens_css(&dir).unwrap();
    assert_eq!(said, format!("wrote {}\n", dir.join(TOKENS_CSS).display()));
    assert_eq!(
        std::fs::read_to_string(dir.join(TOKENS_CSS)).unwrap(),
        tokens_css(TOKENS).unwrap()
    );
    std::fs::write(dir.join(TOKENS_JSON), "[]").unwrap();
    let error = write_tokens_css(&dir).unwrap_err().text();
    assert!(
        error.starts_with(&format!(
            "{}: reading the tokens",
            dir.join(TOKENS_JSON).display()
        )),
        "{error}"
    );
}
