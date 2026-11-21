use super::*;

const TOKENS: &str = r##"{
  "name": "demo",
  "color": {
    "themes": [{ "id": "dark", "name": "Dark" }, { "id": "light", "name": "Light" }],
    "tokens": [
      { "name": "background", "value": { "dark": "#111111", "light": "#EEEEEE" } },
      { "name": "text", "value": { "dark": "#EEEEEE", "light": "#111111" } },
      { "name": "accent-blue", "value": "#0000FF" },
      { "name": "accent-red", "value": "#FF0000" }
    ]
  },
  "type": { "families": {}, "groups": [] }
}"##;

const COLORS: &str = r#"{
  "ground": "background",
  "groups": [
    { "name": "Surfaces", "about": "What sits behind.",
      "clusters": [{ "colors": ["background"] }, { "colors": ["text"], "text": true }],
      "scene": "surfaces" },
    { "name": "Accents", "about": "The accents.",
      "table": { "columns": [{ "label": "text", "text": true }, { "label": "fill" }],
        "rows": [{ "label": "blue", "colors": ["accent-blue", null] },
                 { "label": "red", "colors": ["accent-red", null] }] } }
  ]
}"#;

#[test]
fn the_palette_has_the_themes_of_the_tokens_and_the_groups_in_their_order() {
    let palette = palette(COLORS, TOKENS).unwrap();
    assert_eq!(
        palette.themes,
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
    let names: Vec<(&str, Vec<&str>)> = palette
        .colors
        .groups
        .iter()
        .map(|group| (group.name.as_str(), group.names().collect()))
        .collect();
    assert_eq!(
        names,
        [
            ("Surfaces", vec!["background", "text"]),
            ("Accents", vec!["accent-blue", "accent-red"]),
        ]
    );
    assert_eq!(palette.colors.groups[0].scene.as_deref(), Some("surfaces"));
}

#[test]
fn a_palette_that_leaves_a_color_out_or_names_one_twice_or_none_says_every_problem() {
    let broken = COLORS
        .replace(r#""ground": "background""#, r#""ground": "paper""#)
        .replace(
            r#"{ "colors": ["text"], "text": true }"#,
            r#"{ "colors": ["accent-blue", "ink"] }"#,
        )
        .replace(r#"["accent-red", null]"#, r#"["accent-red"]"#)
        .replace(r#""scene": "surfaces""#, r#""scene": "../up""#);
    assert_eq!(
        palette(&broken, TOKENS).unwrap_err().text(),
        "ground paper is no color of the tokens; \
         group Surfaces names the scene \"../up\", which is no name of letters, digits and -; \
         group Surfaces names ink, which is no color of the tokens; \
         row red of group Accents has 1 entries for 2 columns; \
         color text is in no group; \
         color accent-blue is in more than one group: Surfaces, Accents"
    );
    let empty = COLORS.replace(
        r#""clusters": [{ "colors": ["background"] }, { "colors": ["text"], "text": true }],"#,
        "",
    );
    let error = palette(&empty, TOKENS).unwrap_err().text();
    assert!(
        error.starts_with("group Surfaces shows no swatch; "),
        "{error}"
    );
    let error = palette(
        r#"{ "ground": "background", "groups": [], "extra": 1 }"#,
        TOKENS,
    )
    .unwrap_err()
    .text();
    assert!(
        error.starts_with("reading the color groups: unknown field `extra`"),
        "{error}"
    );
}

#[test]
fn a_scene_names_the_colors_its_elements_point_at_in_order() {
    assert_eq!(
        scene_colors(
            r#"<div data-color="surface"><span data-color="text" data-point="below">A</span></div>"#
        ),
        ["surface", "text"]
    );
    assert!(scene_colors("<div></div>").is_empty());
}
