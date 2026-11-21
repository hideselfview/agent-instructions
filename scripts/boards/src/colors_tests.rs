use super::*;
use crate::tokens::Theme;

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
            Theme {
                id: "dark".into(),
                name: "Dark".into()
            },
            Theme {
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
        )
        .unwrap(),
        ["surface", "text"]
    );
    assert!(scene_colors("<div></div>").unwrap().is_empty());
    assert_eq!(
        scene_colors(r#"<div data-color="surface>A</div>"#)
            .unwrap_err()
            .text(),
        "a data-color mark has no closing quote: data-color=\"surface>A</div>"
    );
}

/// Tokens with a chosen accent and two accents a person can choose, the
/// chosen one red.
const SWITCH_TOKENS: &str = r##"{
  "name": "demo",
  "color": {
    "themes": [{ "id": "dark", "name": "Dark" }, { "id": "light", "name": "Light" }],
    "tokens": [
      { "name": "background", "value": { "dark": "#111111", "light": "#EEEEEE" } },
      { "name": "accent", "value": { "dark": "#FF8888", "light": "#AA0000" } },
      { "name": "accent-tint", "value": { "dark": "rgba(255, 136, 136, 0.14)", "light": "rgba(170, 0, 0, 0.14)" } },
      { "name": "edge", "value": { "dark": "#ff8888", "light": "#000000" } },
      { "name": "accent-blue", "value": { "dark": "#8888FF", "light": "#0000AA" } },
      { "name": "accent-red", "value": { "dark": "#FF8888", "light": "#AA0000" } }
    ]
  },
  "opacity": { "tokens": [{ "name": "opacity-tint", "value": "0.14" }] },
  "type": { "families": {}, "groups": [] }
}"##;

const SWITCH_COLORS: &str = r#"{
  "ground": "background",
  "comparison": { "name": "Every accent", "about": "Each accent.", "scene": "every", "switch": "Accent" },
  "groups": [
    { "name": "Chosen", "about": "The chosen accent.",
      "clusters": [{ "colors": ["background", "accent", "accent-tint", "edge"] }] },
    { "name": "Accents", "about": "The accents.",
      "table": { "columns": [{ "label": "text", "sets": "accent", "dot": true }],
        "rows": [{ "label": "blue", "colors": ["accent-blue"] },
                 { "label": "red", "colors": ["accent-red"] }] },
      "switch": { "name": "Accent", "derived": [
        { "color": "accent-tint", "from": "accent", "opacity": "opacity-tint" },
        { "color": "edge", "from": "accent", "themes": ["dark"] }] } }
  ]
}"#;

#[test]
fn a_table_that_names_a_switch_gives_each_row_the_declarations_that_draw_it() {
    let palette = palette(SWITCH_COLORS, SWITCH_TOKENS).unwrap();
    let [accent] = palette.switches.as_slice() else {
        panic!("{:?}", palette.switches);
    };
    assert_eq!(accent.name, "Accent");
    assert_eq!(accent.chosen, 1);
    let blue = &accent.options[0];
    assert_eq!(blue.label, "blue");
    assert_eq!(blue.dot.as_deref(), Some("#8888FF"));
    assert_eq!(
        blue.declarations,
        [
            "--accent: #8888FF; --accent-tint: color-mix(in srgb, #8888FF 14%, transparent); --edge: #8888FF",
            "--accent: #0000AA; --accent-tint: color-mix(in srgb, #0000AA 14%, transparent)",
        ]
    );
}

#[test]
fn a_switch_that_sets_no_color_derives_wrongly_or_holds_no_row_of_the_tokens_says_every_problem() {
    let broken = SWITCH_COLORS
        .replace(
            r#""from": "accent", "themes": ["dark"]"#,
            r#""from": "edge", "themes": ["dim"], "opacity": "2""#,
        )
        .replace(r#""switch": "Accent" }"#, r#""switch": "Hue" }"#);
    assert_eq!(
        palette(&broken, SWITCH_TOKENS).unwrap_err().text(),
        "group Accents derives edge from edge, which the table sets no column of; \
         group Accents derives edge at the opacity 2, which is neither an opacity token nor a number from 0 to 1; \
         group Accents derives edge in the theme dim, which the tokens lack; \
         the comparison names the switch Hue, which no table names"
    );
    let unheld = SWITCH_TOKENS.replace(
        r##""accent", "value": { "dark": "#FF8888""##,
        r##""accent", "value": { "dark": "#FF0000""##,
    );
    assert_eq!(
        palette(SWITCH_COLORS, &unheld).unwrap_err().text(),
        "no row of group Accents holds the values that the tokens give accent"
    );
    let unset = SWITCH_COLORS.replace(r#""sets": "accent", "#, "");
    assert_eq!(
        palette(&unset, SWITCH_TOKENS).unwrap_err().text(),
        "column text of group Accents sets no color, and the group names a switch; \
         group Accents derives accent-tint from accent, which the table sets no column of; \
         group Accents derives edge from accent, which the table sets no column of"
    );
}
