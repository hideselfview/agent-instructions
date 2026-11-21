//! A design system's color groups, `design/system/colors.json`: every color
//! of its tokens.json in the group of its role, each group with a line that
//! introduces it, its swatches (clusters of colors that belong together, a
//! table of rows and columns, or both) and the scene that draws its colors in
//! use, `design/system/colors/<scene>.html`. A scene marks each element that a
//! swatch chip points at with `data-color="<color>"`.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::tokens::color_names;
use crate::{Context, Result, Source, TOKENS_JSON, fail};

pub const COLORS_JSON: &str = "design/system/colors.json";

/// The groups, as colors.json lists them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    /// The color every swatch is laid over, so that a see-through color
    /// shows as it does on screen.
    pub ground: String,
    pub groups: Vec<Group>,
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub label: String,
    #[serde(default)]
    pub text: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub label: String,
    pub colors: Vec<Option<String>>,
}

/// A theme of the tokens: the id its `data-theme` names, and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeName {
    pub id: String,
    pub name: String,
}

/// The color groups of a design system, with its themes.
#[derive(Debug)]
pub struct Palette {
    pub themes: Vec<ThemeName>,
    /// Every color of the tokens, in the file's order.
    pub names: Vec<String>,
    pub colors: Colors,
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
/// swatch, a table row has not one entry per column, or a scene's name is no
/// file name of letters, digits and `-`.
pub fn palette(colors_json: &str, tokens_json: &str) -> Result<Palette> {
    let colors: Colors =
        serde_json::from_str(colors_json).context(|| "reading the color groups".to_string())?;
    let (themes, names) = color_names(tokens_json)?;
    let mut problems = Vec::new();
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
        if let Some(table) = &group.table {
            for row in &table.rows {
                if row.colors.len() != table.columns.len() {
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
        if let Some(scene) = &group.scene
            && (scene.is_empty() || !scene.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        {
            problems.push(format!(
                "group {} names the scene {scene:?}, which is no name of letters, digits and -",
                group.name
            ));
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
    if !problems.is_empty() {
        fail!("{}", problems.join("; "));
    }
    Ok(Palette {
        themes,
        names,
        colors,
    })
}

/// Where the scene named <scene> is.
pub fn scene_path(scene: &str) -> String {
    format!("design/system/colors/{scene}.html")
}

/// The colors that the elements of the scene <html> point at
/// (`data-color="<color>"`), in the order the scene has them.
pub fn scene_colors(html: &str) -> Vec<&str> {
    const MARK: &str = "data-color=\"";
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find(MARK) {
        let after = &rest[start + MARK.len()..];
        let Some(end) = after.find('"') else { break };
        found.push(&after[..end]);
        rest = &after[end..];
    }
    found
}

#[cfg(test)]
#[path = "colors_tests.rs"]
mod tests;
