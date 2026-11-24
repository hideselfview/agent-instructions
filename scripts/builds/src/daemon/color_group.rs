//! A frame of the Designs page's Colors section: the <n>th group of the
//! design system's colors.json (`boards::colors`,
//! `/designs/<project>/<commit>/colors/<n>`), or its comparison
//! (`.../colors/comparison`), drawn with the commit's
//! own tokens.css, bundle.css and bundle.js, and React. A scene draws a
//! component of the design system with an element that names it by its
//! global name and gives its props as JSON
//! (`data-component="Bae.StatusGlyph" data-props='{"tone": "danger"}'`),
//! which the script mounts before it places the chips. A group's frame shows
//! its swatches, each split into one part per theme of the tokens and laid
//! over the colors' ground, and its scene. The comparison's frame shows its
//! scene once for each option of its switch, under the option's name. Its
//! script (`color_group.js`) puts a swatch chip for each element of a
//! group's scene that names a color (`data-color`) in a column at the left,
//! draws a line from the chip to the element, and tells the page the height
//! the frame takes. The page sends the frame the CSS of the options its
//! switches pick and the theme the scenes draw in, and the script redraws
//! with them.
//!
//! The dashboard's palette comes after the design system's stylesheets, so
//! the frame's own words and lines keep the dashboard's colors on `:root`,
//! and every drawing in the design system's colors sits in an element that
//! names its theme (`data-theme`), where the tokens set every color.

use std::path::Path;

use boards::colors::{self, Cluster, Group, Palette, Table};
use boards::{Source, escape_html as escape};

use crate::design_render::{REACT, fill, script_safe};
use crate::fail;
use crate::fail::Result;

const FRAME_PAGE: &str = include_str!("color_group.html");
const FRAME_SCRIPT: &str = include_str!("color_group.js");

/// The frame of group <n> of the colors of <from>; a commit's design system
/// is generated into <cache>. Fails when the color groups cannot be read or
/// have no group <n>. A scene that cannot be read, or that points at a name
/// that is no color, shows as that problem in the scene's place.
pub fn frame(from: Source, n: usize, cache: &Path) -> Result<String> {
    from.check()?;
    let palette = colors::read(&from)?;
    let Some(group) = palette.colors.groups.get(n) else {
        fail!(
            "{} has {} groups, so no group {n}",
            from.label(colors::COLORS_JSON),
            palette.colors.groups.len()
        );
    };
    let scene = match &group.scene {
        None => String::new(),
        Some(scene) => match scene_html(&from, &palette, scene) {
            Ok(html) => in_use(&palette, &html),
            Err(problem) => problem_html(&problem),
        },
    };
    page(&from, &palette, &swatches(&palette, group), &scene, cache)
}

/// The frame of the comparison of the colors of <from>: its scene once for
/// each option of its switch, each drawn in the option's colors; a commit's
/// design system is generated into <cache>. Fails when the color groups
/// cannot be read or have no comparison. A scene that cannot be read shows
/// as that problem in its place.
pub fn comparison(from: Source, cache: &Path) -> Result<String> {
    from.check()?;
    let palette = colors::read(&from)?;
    let Some(comparison) = &palette.colors.comparison else {
        fail!("{} has no comparison", from.label(colors::COLORS_JSON));
    };
    let switch = palette
        .switches
        .iter()
        .find(|switch| switch.name == comparison.switch)
        .expect("colors::palette checks that the comparison names a switch");
    let scenes = match scene_html(&from, &palette, &comparison.scene) {
        Err(problem) => problem_html(&problem),
        Ok(html) => {
            let theme = &palette.themes.first().id;
            let mut css = String::new();
            let mut drawn = String::new();
            for (at, option) in switch.options.iter().enumerate() {
                for (theme, declarations) in palette.themes.iter().zip(&option.declarations) {
                    css.push_str(&format!(
                        ".cg-scene.cg-opt-{at}[data-theme=\"{}\"] {{ {declarations} }}\n",
                        theme.id
                    ));
                }
                drawn.push_str(&format!(
                    "<div><div class=\"cg-label\">{}</div><div class=\"cg-scene cg-opt-{at}\" data-theme=\"{}\">{html}</div></div>",
                    escape(&option.label),
                    escape(theme)
                ));
            }
            format!("<style>{css}</style><div class=\"cg-row\">{drawn}</div>")
        }
    };
    page(&from, &palette, "", &scenes, cache)
}

fn problem_html(problem: &str) -> String {
    format!("<div class=\"cg-problem\">{}</div>", escape(problem))
}

/// The frame's page, with <swatches> and <scene> in it.
fn page(
    from: &Source,
    palette: &Palette,
    swatches: &str,
    scene: &str,
    cache: &Path,
) -> Result<String> {
    let [tokens_css, bundle_css, bundle_js] = from.board_design_system(cache)?;
    let themes: Vec<&str> = palette
        .themes
        .iter()
        .map(|theme| theme.id.as_str())
        .collect();
    Ok(fill(
        FRAME_PAGE,
        &[
            ("TOKENS_CSS", &tokens_css),
            ("BUNDLE_CSS", &bundle_css),
            ("REACT", REACT),
            ("BUNDLE_JS", &script_safe(&bundle_js)),
            ("PALETTE", super::designs::PALETTE),
            ("THEMES", &escape(&themes.join(" "))),
            ("GROUND", &escape(&palette.colors.ground)),
            ("SWATCHES", swatches),
            ("USE", scene),
            ("SCRIPT", FRAME_SCRIPT),
        ],
    ))
}

/// The scene named <scene> of <from>, or why it can't be drawn: it can't be
/// read, a `data-color` mark in it has no closing quote, or an element of
/// it points at a name that is no color of <palette>.
fn scene_html(
    from: &Source,
    palette: &Palette,
    scene: &str,
) -> std::result::Result<String, String> {
    let rel = colors::scene_path(scene);
    let html = from.read(&rel, "the scene").map_err(|error| error.text())?;
    let named =
        colors::scene_colors(&html).map_err(|error| format!("{}: {error}", from.label(&rel)))?;
    let unknown: Vec<&str> = named
        .into_iter()
        .filter(|name| !palette.names.iter().any(|known| known == name))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "{} points at {}, which the tokens have no color for",
            from.label(&rel),
            unknown.join(", ")
        ));
    }
    Ok(html)
}

/// The group's swatches: its table, then its clusters.
fn swatches(palette: &Palette, group: &Group) -> String {
    let mut out = String::new();
    if let Some(table) = &group.table {
        out.push_str(&table_html(palette, table));
    }
    if !group.clusters.is_empty() {
        out.push_str("<div class=\"cg-clusters\">");
        for cluster in &group.clusters {
            out.push_str(&cluster_html(palette, cluster));
        }
        out.push_str("</div>");
    }
    out
}

fn cluster_html(palette: &Palette, cluster: &Cluster) -> String {
    let swatches: String = cluster
        .colors
        .iter()
        .map(|name| {
            format!(
                "<div class=\"cg-sw\" title=\"{name}\"><div class=\"cg-sq\">{}</div><span class=\"cg-nm\">{name}</span></div>",
                halves(palette, name, cluster.text),
                name = escape(name)
            )
        })
        .collect();
    format!("<div class=\"cg-cluster\">{swatches}</div>")
}

fn table_html(palette: &Palette, table: &Table) -> String {
    let mut out = format!(
        "<div class=\"cg-table\" style=\"grid-template-columns: auto repeat({}, minmax(72px, max-content))\"><div></div>",
        table.columns.len()
    );
    for column in &table.columns {
        out.push_str(&format!(
            "<div class=\"cg-th cg-nm\">{}</div>",
            escape(&column.label)
        ));
    }
    for row in &table.rows {
        out.push_str(&format!(
            "<div class=\"cg-rh\">{}</div>",
            escape(&row.label)
        ));
        for (color, column) in row.colors.iter().zip(&table.columns) {
            match color {
                Some(name) => out.push_str(&format!(
                    "<div class=\"cg-cell\" title=\"{}\"><div class=\"cg-sq\">{}</div></div>",
                    escape(name),
                    halves(palette, name, column.text)
                )),
                None => out.push_str("<div class=\"cg-cell cg-none\"></div>"),
            }
        }
    }
    out.push_str("</div>");
    out
}

/// One part of a swatch per theme: the color laid over the ground, or for a
/// color of text (<text>), words in the color on the ground.
fn halves(palette: &Palette, name: &str, text: bool) -> String {
    let (name, ground) = (escape(name), escape(&palette.colors.ground));
    palette
        .themes
        .iter()
        .map(|theme| {
            let id = escape(&theme.id);
            if text {
                format!(
                    "<span data-theme=\"{id}\" class=\"cg-tx\" style=\"color: var(--{name}); background: var(--{ground})\">Aa</span>"
                )
            } else {
                format!(
                    "<span data-theme=\"{id}\" style=\"background: linear-gradient(var(--{name}), var(--{name})), var(--{ground})\"></span>"
                )
            }
        })
        .collect()
}

/// The scene <html>, in the first theme until the page picks one, beside
/// the column the script puts the chips in.
fn in_use(palette: &Palette, html: &str) -> String {
    let theme = &palette.themes.first().id;
    format!(
        "<div class=\"cg-use\"><div class=\"cg-gutter\"></div><div class=\"cg-scene\" data-theme=\"{}\">{html}</div></div>",
        escape(theme)
    )
}

#[cfg(test)]
#[path = "color_group_tests.rs"]
mod tests;
