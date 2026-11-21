//! A design system's color groups, `design/system/colors.json`: every color
//! of its tokens.json in the group of its role, each group with a line that
//! introduces it, its swatches (clusters of colors that belong together, a
//! table of rows and columns, or both) and the scene that draws its colors in
//! use, `design/system/colors/<scene>.html`. A scene marks each element that a
//! swatch chip points at with `data-color="<color>"`.
//!
//! A table can name a switch: the person picks one of its rows, and each
//! column's color takes the place of the color the column sets, with the
//! colors that follow those (`derived`). tokens.json holds one row's values in
//! the colors the table sets, and that row is the switch's first choice. The
//! comparison draws one scene once for each option of a switch.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::tokens::{Themes, color_values};
use crate::{Context, Result, Source, TOKENS_JSON, fail};

pub const COLORS_JSON: &str = "design/system/colors.json";

/// The groups, as colors.json lists them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    /// The color every swatch is laid over, so that a see-through color
    /// shows as it does on screen.
    pub ground: String,
    pub comparison: Option<Comparison>,
    pub groups: Vec<Group>,
}

/// A scene drawn once for each option of a switch, side by side.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    pub name: String,
    pub about: String,
    pub scene: String,
    /// The name of the switch whose options the scene is drawn in.
    pub switch: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub about: String,
    /// The table, drawn before the clusters.
    pub table: Option<Table>,
    #[serde(default)]
    pub clusters: Vec<Cluster>,
    /// The name of the group's scene file, letters, digits and `-`.
    pub scene: Option<String>,
}

/// The switch a table's rows make, as colors.json names it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchSpec {
    pub name: String,
    #[serde(default)]
    pub derived: Vec<Derived>,
}

/// A color that follows a color the switch sets: the same color, at an
/// opacity when it names one (an opacity token or a number), in every theme
/// or only in the themes it names.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derived {
    pub color: String,
    pub from: String,
    pub opacity: Option<String>,
    pub themes: Option<Vec<String>>,
}

/// Colors whose swatches sit side by side.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cluster {
    pub colors: Vec<String>,
    /// The swatches draw words in the color, for a color of text.
    #[serde(default)]
    pub text: bool,
}

/// Rows of colors, one per column or none.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// The switch the rows make.
    pub switch: Option<SwitchSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub label: String,
    #[serde(default)]
    pub text: bool,
    /// The color whose place the column's color takes when the person picks
    /// its row, in a table that names a switch.
    pub sets: Option<String>,
    /// The column's color, in the first theme, is the dot on its row's
    /// switch button.
    #[serde(default)]
    pub dot: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub label: String,
    pub colors: Vec<Option<String>>,
}

/// The color groups of a design system, with its themes.
#[derive(Debug)]
pub struct Palette {
    pub themes: Themes,
    /// Every color of the tokens, in the file's order.
    pub names: Vec<String>,
    pub colors: Colors,
    /// The switches the tables make, in the groups' order.
    pub switches: Vec<Switch>,
}

/// A switch the page draws: one button per option.
#[derive(Debug, Serialize)]
pub struct Switch {
    pub name: String,
    pub options: Vec<Choice>,
    /// The option whose colors tokens.json holds.
    pub chosen: usize,
}

/// One option of a switch.
#[derive(Debug, Serialize)]
pub struct Choice {
    pub label: String,
    /// The color of the dot on the option's button, if the table has one.
    pub dot: Option<String>,
    /// The CSS custom properties that draw the option, one list per theme in
    /// the themes' order.
    pub declarations: Vec<String>,
}

impl Group {
    /// Every color the group's swatches show, in their order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let table = self
            .table
            .iter()
            .flat_map(|table| &table.rows)
            .flat_map(|row| row.colors.iter().flatten());
        table
            .chain(self.clusters.iter().flat_map(|cluster| &cluster.colors))
            .map(String::as_str)
    }
}

/// The color groups at <from>, checked against its tokens.json (`palette`).
pub fn read(from: &Source) -> Result<Palette> {
    let colors = from.read(COLORS_JSON, "the color groups")?;
    let tokens = from.read(TOKENS_JSON, "the design system's tokens")?;
    palette(&colors, &tokens)
        .map_err(|error| crate::Error::new(format!("{}: {error}", from.label(COLORS_JSON))))
}

/// The groups of <colors_json> with the themes of <tokens_json>. Fails,
/// naming every problem, when `ground` or a group names no color of the
/// tokens, a color of the tokens is in no group or in two, a group shows no
/// swatch, a table row has not one entry per column, a scene's name is no
/// file name of letters, digits and `-`, a switch is wrong (`switch`), or the
/// comparison names no switch.
pub fn palette(colors_json: &str, tokens_json: &str) -> Result<Palette> {
    let colors: Colors =
        serde_json::from_str(colors_json).context(|| "reading the color groups".to_string())?;
    let tokens = color_values(tokens_json)?;
    let names: Vec<String> = tokens.colors.iter().map(|(name, _)| name.clone()).collect();
    let mut problems = Vec::new();
    let mut switches = Vec::new();
    if !names.contains(&colors.ground) {
        problems.push(format!(
            "ground {} is no color of the tokens",
            colors.ground
        ));
    }
    let mut groups_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for group in &colors.groups {
        if group.table.is_none() && group.clusters.is_empty() {
            problems.push(format!("group {} shows no swatch", group.name));
        }
        let mut rows_fit = true;
        if let Some(table) = &group.table {
            for row in &table.rows {
                if row.colors.len() != table.columns.len() {
                    rows_fit = false;
                    problems.push(format!(
                        "row {} of group {} has {} entries for {} columns",
                        row.label,
                        group.name,
                        row.colors.len(),
                        table.columns.len()
                    ));
                }
            }
        }
        if let Some(scene) = &group.scene {
            check_scene(&format!("group {}", group.name), scene, &mut problems);
        }
        if let Some(table) = &group.table
            && let Some(spec) = &table.switch
            && rows_fit
            && let Some(made) = switch(&group.name, spec, table, &tokens, &mut problems)
        {
            switches.push(made);
        }
        for name in group.names() {
            if !names.iter().any(|known| known == name) {
                problems.push(format!(
                    "group {} names {name}, which is no color of the tokens",
                    group.name
                ));
            }
            groups_of.entry(name).or_default().push(&group.name);
        }
    }
    for name in &names {
        match groups_of.get(name.as_str()).map(Vec::as_slice) {
            None | Some([]) => problems.push(format!("color {name} is in no group")),
            Some([_]) => {}
            Some(groups) => problems.push(format!(
                "color {name} is in more than one group: {}",
                groups.join(", ")
            )),
        }
    }
    let named: Vec<&str> = colors
        .groups
        .iter()
        .filter_map(|group| {
            group
                .table
                .as_ref()?
                .switch
                .as_ref()
                .map(|spec| spec.name.as_str())
        })
        .collect();
    for (at, name) in named.iter().enumerate() {
        if named[..at].contains(name) {
            problems.push(format!("two tables name the switch {name}"));
        }
    }
    if let Some(comparison) = &colors.comparison {
        check_scene("the comparison", &comparison.scene, &mut problems);
        if !named.contains(&comparison.switch.as_str()) {
            problems.push(format!(
                "the comparison names the switch {}, which no table names",
                comparison.switch
            ));
        }
    }
    if !problems.is_empty() {
        fail!("{}", problems.join("; "));
    }
    Ok(Palette {
        themes: tokens.themes,
        names,
        colors,
        switches,
    })
}

/// A problem when <scene>, which <owner> names, is no file name of letters,
/// digits and `-`.
fn check_scene(owner: &str, scene: &str, problems: &mut Vec<String>) {
    if scene.is_empty() || !scene.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        problems.push(format!(
            "{owner} names the scene {scene:?}, which is no name of letters, digits and -"
        ));
    }
}

/// The switch that the rows of <table> in group <group> make, each row with
/// one entry per column, or none, with a problem for each column that sets
/// no color of the tokens, each empty cell, each derived color that is no
/// color, follows no color the table sets, names an opacity that is neither
/// a token nor a number from 0 to 1, or names a theme the tokens lack, and
/// when no row holds the values that tokens.json gives the colors the table
/// sets.
fn switch(
    group: &str,
    spec: &SwitchSpec,
    table: &Table,
    tokens: &crate::tokens::ColorValues,
    problems: &mut Vec<String>,
) -> Option<Switch> {
    let before = problems.len();
    let value = |name: &str| {
        tokens
            .colors
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, v)| v)
    };
    let mut sets = Vec::new();
    for column in &table.columns {
        match column.sets.as_deref() {
            Some(name) if value(name).is_some() => sets.push(name),
            Some(name) => problems.push(format!(
                "column {} of group {group} sets {name}, which is no color of the tokens",
                column.label
            )),
            None => problems.push(format!(
                "column {} of group {group} sets no color, and the group names a switch",
                column.label
            )),
        }
    }
    let mut opacities = Vec::new();
    for derived in &spec.derived {
        if value(&derived.color).is_none() {
            problems.push(format!(
                "group {group} derives {}, which is no color of the tokens",
                derived.color
            ));
        }
        let column = sets.iter().position(|name| *name == derived.from);
        if column.is_none() {
            problems.push(format!(
                "group {group} derives {} from {}, which the table sets no column of",
                derived.color, derived.from
            ));
        }
        let opacity = match &derived.opacity {
            None => None,
            Some(opacity) => {
                let number = tokens.scalars.get(opacity).unwrap_or(opacity);
                match number.parse::<f64>() {
                    Ok(number) if (0.0..=1.0).contains(&number) => Some(number),
                    _ => {
                        problems.push(format!(
                            "group {group} derives {} at the opacity {opacity}, which is neither an opacity token nor a number from 0 to 1",
                            derived.color
                        ));
                        None
                    }
                }
            }
        };
        for theme in derived.themes.iter().flatten() {
            if !tokens.themes.iter().any(|known| &known.id == theme) {
                problems.push(format!(
                    "group {group} derives {} in the theme {theme}, which the tokens lack",
                    derived.color
                ));
            }
        }
        opacities.push((column, opacity));
    }
    let mut options = Vec::new();
    let mut chosen = None;
    for row in &table.rows {
        let cells: Vec<&Vec<String>> = row
            .colors
            .iter()
            .filter_map(|cell| {
                let found = cell.as_deref().and_then(value);
                if found.is_none() {
                    problems.push(format!(
                        "row {} of group {group} has a cell with no color, and the group names a switch",
                        row.label
                    ));
                }
                found
            })
            .collect();
        if problems.len() > before {
            continue;
        }
        let declarations = tokens
            .themes
            .iter()
            .enumerate()
            .map(|(at, theme)| {
                let mut out: Vec<String> = sets
                    .iter()
                    .zip(&cells)
                    .map(|(name, cell)| format!("--{name}: {}", cell[at]))
                    .collect();
                for (derived, (column, opacity)) in spec.derived.iter().zip(&opacities) {
                    let in_theme = derived
                        .themes
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&theme.id));
                    let Some(column) = column.filter(|_| in_theme) else {
                        continue;
                    };
                    let from = &cells[column][at];
                    out.push(match opacity {
                        None => format!("--{}: {from}", derived.color),
                        Some(opacity) => format!(
                            "--{}: color-mix(in srgb, {from} {}%, transparent)",
                            derived.color,
                            (opacity * 1000.0).round() / 10.0
                        ),
                    });
                }
                out.join("; ")
            })
            .collect();
        let holds = sets.iter().zip(&cells).all(|(name, cell)| {
            value(name).is_some_and(|set| {
                set.iter()
                    .zip(cell.iter())
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
            })
        });
        if holds && chosen.is_none() {
            chosen = Some(options.len());
        }
        let dot = table.columns.iter().position(|column| column.dot);
        options.push(Choice {
            label: row.label.clone(),
            dot: dot.map(|dot| cells[dot][0].clone()),
            declarations,
        });
    }
    if problems.len() > before {
        return None;
    }
    let Some(chosen) = chosen else {
        problems.push(format!(
            "no row of group {group} holds the values that the tokens give {}",
            sets.join(", ")
        ));
        return None;
    };
    Some(Switch {
        name: spec.name.clone(),
        options,
        chosen,
    })
}

/// Where the scene named <scene> is.
pub fn scene_path(scene: &str) -> String {
    format!("design/system/colors/{scene}.html")
}

/// The colors that the elements of the scene <html> point at
/// (`data-color="<color>"`), in the order the scene has them. Fails when a
/// `data-color` mark has no closing quote.
pub fn scene_colors(html: &str) -> Result<Vec<&str>> {
    const MARK: &str = "data-color=\"";
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find(MARK) {
        let after = &rest[start + MARK.len()..];
        let Some(end) = after.find('"') else {
            fail!(
                "a data-color mark has no closing quote: {MARK}{}",
                after.chars().take(40).collect::<String>()
            );
        };
        found.push(&after[..end]);
        rest = &after[end..];
    }
    Ok(found)
}

#[cfg(test)]
#[path = "colors_tests.rs"]
mod tests;
