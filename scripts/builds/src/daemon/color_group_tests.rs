use super::*;
use crate::testing::{Scratch, design_system};
use serde_json::json;

/// A worktree whose design system has two themes, three colors in two
/// groups (a table of accents, and surfaces with a scene) and the bundle
/// stylesheet `.bundle { color: red; }`.
fn colors_dir(scratch: &Scratch, scene: &str) -> std::path::PathBuf {
    let dir = scratch.dir.join("repo");
    design_system(&dir, "1", ".bundle { color: red; }", "window.Demo = {};");
    let write = |rel: &str, text: &str| {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "design/system/tokens.json",
        &json!({
            "name": "demo",
            "color": { "themes": [{ "id": "dark", "name": "Dark" }, { "id": "light", "name": "Light" }],
                "tokens": [
                    { "name": "bg", "value": { "dark": "#111111", "light": "#EEEEEE" } },
                    { "name": "ink", "value": { "dark": "#EEEEEE", "light": "#111111" } },
                    { "name": "accent-blue", "value": "#0000FF" },
                ] },
            "type": { "families": {}, "groups": [] },
        })
        .to_string(),
    );
    write(
        "design/system/colors.json",
        &json!({
            "ground": "bg",
            "groups": [
                { "name": "Surfaces", "about": "Behind.", "scene": "surfaces",
                  "clusters": [{ "colors": ["bg"] }, { "colors": ["ink"], "text": true }] },
                { "name": "Accents", "about": "Chosen.",
                  "table": { "columns": [{ "label": "text", "text": true }, { "label": "fill" }],
                    "rows": [{ "label": "blue", "colors": ["accent-blue", null] }] } },
            ],
        })
        .to_string(),
    );
    write("design/system/colors/surfaces.html", scene);
    dir
}

#[test]
fn a_group_draws_its_swatches_in_every_theme_over_the_ground_and_its_scene_in_the_first() {
    let scratch = Scratch::new("color-group-surfaces");
    let dir = colors_dir(&scratch, "<div data-color=\"bg\">window</div>");
    let page = frame(Source::Dir(&dir), 0, &scratch.places().design_dirs()).unwrap();
    assert!(!page.contains("@@"), "every marker is filled");
    let order: Vec<usize> = [
        "--bg: #111111",
        ".bundle { color: red; }",
        "react.production.min.js",
        "window.Demo = {};",
        "--line:",
        "data-themes=\"dark light\" data-ground=\"bg\"",
        "<span data-theme=\"dark\" style=\"background: linear-gradient(var(--bg), var(--bg)), var(--bg)\"></span>\
         <span data-theme=\"light\" style=\"background: linear-gradient(var(--bg), var(--bg)), var(--bg)\"></span>",
        "<span class=\"cg-nm\">bg</span>",
        "<span data-theme=\"dark\" class=\"cg-tx\" style=\"color: var(--ink); background: var(--bg)\">Aa</span>",
        "<div class=\"cg-use\"><div class=\"cg-gutter\"></div><div class=\"cg-scene\" data-theme=\"dark\"><div data-color=\"bg\">window</div></div></div>",
        "chip.className = \"cg-chip\"",
    ]
    .iter()
    .map(|part| page.find(part).unwrap_or_else(|| panic!("{part} in {page}")))
    .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
}

#[test]
fn a_table_draws_one_row_per_entry_with_a_gap_where_a_row_has_no_color() {
    let scratch = Scratch::new("color-group-table");
    let dir = colors_dir(&scratch, "");
    let page = frame(Source::Dir(&dir), 1, &scratch.places().design_dirs()).unwrap();
    for part in [
        "grid-template-columns: auto repeat(2, minmax(72px, max-content))",
        "<div class=\"cg-th cg-nm\">text</div><div class=\"cg-th cg-nm\">fill</div>",
        "<div class=\"cg-rh\">blue</div><div class=\"cg-cell\" title=\"accent-blue\">",
        "class=\"cg-tx\" style=\"color: var(--accent-blue); background: var(--bg)\"",
        "<div class=\"cg-cell cg-none\"></div>",
    ] {
        assert!(page.contains(part), "{part} in {page}");
    }
    assert!(
        !page.contains("<div class=\"cg-use\">"),
        "a group with no scene draws none"
    );
}

#[test]
fn a_scene_that_points_at_no_color_shows_why_and_a_missing_group_fails() {
    let scratch = Scratch::new("color-group-problems");
    let dir = colors_dir(
        &scratch,
        "<b data-color=\"paper\">x</b><i data-color=\"ink\">y</i>",
    );
    let cache = scratch.places().design_dirs();
    let page = frame(Source::Dir(&dir), 0, &cache).unwrap();
    assert!(
        page.contains("<div class=\"cg-problem\">")
            && page.contains(
                "design/system/colors/surfaces.html points at paper, which the tokens have no color for</div>"
            ),
        "{page}"
    );
    assert!(!page.contains("<div class=\"cg-use\">"), "{page}");
    let error = frame(Source::Dir(&dir), 2, &cache).unwrap_err().text();
    assert!(
        error.ends_with("design/system/colors.json has 2 groups, so no group 2"),
        "{error}"
    );
}

#[test]
fn the_comparison_draws_its_scene_once_per_option_in_the_options_colors() {
    let scratch = Scratch::new("color-group-comparison");
    let dir = colors_dir(&scratch, "<b style=\"color: var(--accent)\">Show all</b>");
    let cache = scratch.places().design_dirs();
    let error = comparison(Source::Dir(&dir), &cache).unwrap_err().text();
    assert!(
        error.ends_with("design/system/colors.json has no comparison"),
        "{error}"
    );
    std::fs::write(
        dir.join("design/system/tokens.json"),
        json!({
            "name": "demo",
            "color": { "themes": [{ "id": "dark", "name": "Dark" }, { "id": "light", "name": "Light" }],
                "tokens": [
                    { "name": "bg", "value": { "dark": "#111111", "light": "#EEEEEE" } },
                    { "name": "accent", "value": "#FF0000" },
                    { "name": "accent-blue", "value": "#0000FF" },
                    { "name": "accent-red", "value": "#FF0000" },
                ] },
            "type": { "families": {}, "groups": [] },
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("design/system/colors.json"),
        json!({
            "ground": "bg",
            "comparison": { "name": "Every accent", "about": "Each.", "scene": "surfaces", "switch": "Accent" },
            "groups": [
                { "name": "Surfaces", "about": "Behind.", "clusters": [{ "colors": ["bg", "accent"] }] },
                { "name": "Accents", "about": "Chosen.",
                  "table": { "columns": [{ "label": "text", "sets": "accent" }],
                    "rows": [{ "label": "blue", "colors": ["accent-blue"] },
                             { "label": "red", "colors": ["accent-red"] }],
                    "switch": { "name": "Accent" } } },
            ],
        })
        .to_string(),
    )
    .unwrap();
    let page = comparison(Source::Dir(&dir), &cache).unwrap();
    assert!(!page.contains("@@"), "every marker is filled");
    let order: Vec<usize> = [
        ".cg-scene.cg-opt-0[data-theme=\"dark\"] { --accent: #0000FF }",
        ".cg-scene.cg-opt-1[data-theme=\"light\"] { --accent: #FF0000 }",
        "<div class=\"cg-row\"><div><div class=\"cg-label\">blue</div><div class=\"cg-scene cg-opt-0\" data-theme=\"dark\"><b style=\"color: var(--accent)\">Show all</b></div></div>",
        "<div class=\"cg-label\">red</div><div class=\"cg-scene cg-opt-1\" data-theme=\"dark\">",
        "colorsReady",
    ]
    .iter()
    .map(|part| page.find(part).unwrap_or_else(|| panic!("{part} in {page}")))
    .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
}
