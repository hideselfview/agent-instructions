//! Each design-system component's capture file. A component that has a
//! preview keeps the code that captures it on a platform in one file,
//! `Components/<Component>.<extension>` under the platform's declare folder
//! (the `shots-declared` line), which declares one shot for each row of the
//! component's preview, named `<platform>.component.<thing>.<state>`. A
//! component whose name starts with a platform (`MacRow`) is that platform's;
//! one whose name starts with none (`Artwork`) is every platform's.
//!
//! The listing names each preview row whose shot the component's capture file
//! does not declare, with the file that declares it instead, and each capture
//! file whose component has no preview on its platform. It fails nothing.
//! Web shots are fixture files that each declare one shot by their name, so
//! web keeps no capture files and the listing leaves it out.

use crate::declared::{self, COMPONENTS, Declared};
use crate::names::{Name, Platform, Previews};
use crate::settings::{Settings, split_design_only};
use std::path::{Path, PathBuf};

/// Every component's capture file and every capture file with no preview.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Listing {
    /// Each component with a preview, once for each platform that draws it,
    /// by name and then platform.
    pub(crate) components: Vec<Component>,
    /// The capture files whose component has no preview on their platform,
    /// by path.
    pub(crate) strays: Vec<Stray>,
    /// The folders that hold a preview and whose name is no component's
    /// name, relative to the repo's top: no capture file can be named for
    /// them.
    pub(crate) unnamed: Vec<String>,
    /// How many components, counted once for each platform, the listing
    /// leaves out because the platform draws them for the design only
    /// (`shots-design-only`): its app has no view their shots could draw.
    pub(crate) design_only: usize,
    /// How many preview rows, counted once for each platform, the listing
    /// leaves out because the platform draws them for the design only
    /// (`shots-design-only Component.row`) while it draws the component's
    /// other rows.
    pub(crate) design_only_rows: usize,
}

/// A component on one platform: its capture file and its preview's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Component {
    pub(crate) name: String,
    pub(crate) platform: Platform,
    /// The capture file, relative to the repo's top.
    pub(crate) file: PathBuf,
    pub(crate) file_exists: bool,
    /// The preview's rows, in the preview's order.
    pub(crate) rows: Vec<Row>,
}

/// A preview row: the shot that draws it and which file declares that shot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) shot: Name,
    pub(crate) place: Place,
}

/// Which file declares a row's shot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Place {
    /// The component's capture file.
    File,
    /// Another harness file, relative to the repo's top.
    Elsewhere(PathBuf),
    /// No harness file.
    Nowhere,
}

/// A capture file whose component has no preview on its platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stray {
    /// The file, relative to the repo's top.
    pub(crate) file: PathBuf,
    /// The file's name without its extension, the component it is named for.
    pub(crate) stem: String,
    pub(crate) why: String,
}

impl Listing {
    /// The sentence that counts every row the components leave out of their
    /// capture files, every capture file with no preview and every preview
    /// folder with no component name.
    pub(crate) fn counts(&self) -> String {
        counts(
            &self.components.iter().collect::<Vec<_>>(),
            self.strays.len(),
            self.unnamed.len(),
            (self.design_only, self.design_only_rows),
        )
    }

    /// Whether a row is left out of its capture file, a capture file has no
    /// preview or a preview folder has no component name.
    pub(crate) fn has_gaps(&self) -> bool {
        !self.strays.is_empty()
            || !self.unnamed.is_empty()
            || self.components.iter().any(|c| c.missing().next().is_some())
    }
}

impl Component {
    /// The rows whose shot the capture file does not declare.
    fn missing(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.place != Place::File)
    }
}

/// The listing for the repo at <root>, with its previews, its settings and
/// the shots its harnesses declare.
pub(crate) fn list(
    root: &Path,
    previews: &Previews,
    settings: &Settings,
    declared: &[Declared],
) -> Result<Listing, String> {
    // Each platform whose harness declares shots in source, with the
    // extension of its files and its declare folder.
    let platforms: Vec<(Platform, &str, &Path)> = settings
        .declared
        .iter()
        .filter_map(|(platform, folder)| {
            declared::source_extension(*platform)
                .map(|extension| (*platform, extension, Path::new(folder)))
        })
        .collect();
    let relative = |path: &Path| path.strip_prefix(root).unwrap_or(path).to_path_buf();

    let mut components = Vec::new();
    let mut design_only = 0;
    let mut design_only_rows = 0;
    if let Some(problem) = design_only_problems(root, previews, settings)?
        .into_iter()
        .next()
    {
        return Err(problem);
    }
    for (name, key) in &previews.components {
        let states = preview_rows(root, name, settings)?;
        let preview = root.join(boards::preview::preview_path(name));
        for (platform, extension, folder) in &platforms {
            if key.platform.is_some_and(|p| p != platform.word()) {
                continue;
            }
            let marked = |row: Option<&str>| {
                settings.design_only.iter().any(|(only, spec)| {
                    only == platform && split_design_only(spec) == (name.as_str(), row)
                })
            };
            if marked(None) {
                design_only += 1;
                continue;
            }
            let file = folder.join(COMPONENTS).join(format!("{name}.{extension}"));
            let path = root.join(&file);
            let mut rows = Vec::new();
            for state in &states {
                if marked(Some(state)) {
                    design_only_rows += 1;
                    continue;
                }
                let shot = crate::names::component_shot(key, *platform, state)
                    .map_err(|e| format!("{}: {e}", preview.display()))?;
                let place = match declared.iter().find(|d| d.name == shot) {
                    Some(d) if d.file == path => Place::File,
                    Some(d) => Place::Elsewhere(relative(&d.file)),
                    None => Place::Nowhere,
                };
                rows.push(Row { shot, place });
            }
            components.push(Component {
                name: name.clone(),
                platform: *platform,
                file_exists: path.is_file(),
                file,
                rows,
            });
        }
    }
    components.sort_by(|a, b| a.name.cmp(&b.name).then(a.platform.cmp(&b.platform)));

    let mut strays = Vec::new();
    for (platform, _, folder) in &platforms {
        for file in
            declared::capture_files(&root.join(folder), *platform).map_err(|e| e.to_string())?
        {
            let stem = file.stem;
            let why = match previews.components.iter().find(|(name, _)| *name == stem) {
                Some((_, key)) => match key.platform {
                    Some(owner) if owner != platform.word() => format!(
                        "{stem} is a component of {owner}, not of {}",
                        platform.word()
                    ),
                    _ => continue,
                },
                None => format!(
                    "no folder {}/{stem} holds a preview.html",
                    boards::preview::COMPONENTS
                ),
            };
            strays.push(Stray {
                file: relative(&file.path),
                stem,
                why,
            });
        }
    }
    strays.sort_by(|a, b| a.file.cmp(&b.file));
    let unnamed = previews
        .unnamed
        .iter()
        .map(|name| format!("{}/{name}", boards::preview::COMPONENTS))
        .collect();
    Ok(Listing {
        components,
        strays,
        unnamed,
        design_only,
        design_only_rows,
    })
}

/// One sentence for each `shots-design-only` name that names a component
/// with no preview on its platform, or a row its component's preview lacks.
pub(crate) fn design_only_problems(
    root: &Path,
    previews: &Previews,
    settings: &Settings,
) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    for (platform, spec) in &settings.design_only {
        let (name, row) = split_design_only(spec);
        let drawn = previews.components.iter().find(|(known, key)| {
            known == name && key.platform.is_none_or(|p| p == platform.word())
        });
        let Some((name, _)) = drawn else {
            problems.push(format!(
                "shots-design-only names {name} on {}, which has no preview there",
                platform.word()
            ));
            continue;
        };
        if let Some(row) = row
            && !preview_rows(root, name, settings)?.contains(&row.to_string())
        {
            problems.push(format!(
                "shots-design-only names {name}.{row} on {}, which is no row of {name}'s preview",
                platform.word()
            ));
        }
    }
    Ok(problems)
}

/// The ids of the rows of component <name>'s preview page in the repo at
/// <root>.
fn preview_rows(root: &Path, name: &str, settings: &Settings) -> Result<Vec<String>, String> {
    let preview = root.join(boards::preview::preview_path(name));
    let html = std::fs::read_to_string(&preview)
        .map_err(|e| format!("cannot read {}: {e}", preview.display()))?;
    Ok(crate::lint::rows(&html, &settings.preview_calls))
}

/// Fails, naming it, when a name in <only> names no component with a preview
/// and no capture file.
pub(crate) fn check_names(listing: &Listing, only: &[String]) -> Result<(), String> {
    for name in only {
        if !listing.components.iter().any(|c| &c.name == name)
            && !listing.strays.iter().any(|s| &s.stem == name)
        {
            return Err(format!(
                "no component {name} has a preview, and no capture file is named {name}"
            ));
        }
    }
    Ok(())
}

/// The listing as lines of text. With no <only>, each component that has a
/// row its capture file does not declare, with those rows and the file that
/// declares each instead, named from the declare folder, then each capture
/// file with no preview. With component names in <only>, those components
/// with every row, so that someone moving one component's shots sees each
/// row's place, and the capture files named for them. With no <only>, the
/// preview folders whose name is no component name come last. The last line
/// counts what the lines show.
pub(crate) fn lines(listing: &Listing, only: &[String]) -> Vec<String> {
    let named = |name: &str| only.iter().any(|n| n == name);
    let components: Vec<&Component> = listing
        .components
        .iter()
        .filter(|c| {
            if only.is_empty() {
                c.missing().next().is_some()
            } else {
                named(&c.name)
            }
        })
        .collect();
    let strays: Vec<&Stray> = listing
        .strays
        .iter()
        .filter(|s| only.is_empty() || named(&s.stem))
        .collect();

    let mut lines = Vec::new();
    for component in &components {
        let file = component.file.display();
        if component.file_exists {
            lines.push(format!(
                "{} on {}: {file}",
                component.name,
                component.platform.word()
            ));
        } else {
            lines.push(format!(
                "{} on {}: {file} does not exist",
                component.name,
                component.platform.word()
            ));
        }
        // The declare folder, which holds the capture file's Components
        // folder: another file is named from it, the shorter for that.
        let folder = component.file.parent().and_then(Path::parent);
        for row in &component.rows {
            let place = match &row.place {
                Place::File if only.is_empty() => continue,
                Place::File => "this file declares it".to_string(),
                Place::Elsewhere(other) => {
                    let shown = folder
                        .and_then(|f| other.strip_prefix(f).ok())
                        .unwrap_or(other);
                    format!("{} declares it, not this file", shown.display())
                }
                Place::Nowhere => "no harness file declares it".to_string(),
            };
            lines.push(format!("  {}: {place}", row.shot));
        }
    }
    if !strays.is_empty() {
        lines.push("Capture files with no preview:".to_string());
        for stray in &strays {
            lines.push(format!("  {}: {}", stray.file.display(), stray.why));
        }
    }
    let unnamed: &[String] = if only.is_empty() {
        &listing.unnamed
    } else {
        &[]
    };
    if !unnamed.is_empty() {
        lines.push(
            "Preview folders whose name is no component name, so no capture file can be named for them:"
                .to_string(),
        );
        for folder in unnamed {
            lines.push(format!("  {folder}"));
        }
    }
    lines.push(counts(
        &components,
        strays.len(),
        unnamed.len(),
        (listing.design_only, listing.design_only_rows),
    ));
    lines
}

/// The sentence that counts the rows <components> leave out of their capture
/// files, the <strays>, the <unnamed> preview folders and the components the
/// listing leaves out because they draw for the design only, <design_only>
/// whole components and <design_only_rows> single rows of the rest.
fn counts(
    components: &[&Component],
    strays: usize,
    unnamed: usize,
    (design_only, design_only_rows): (usize, usize),
) -> String {
    let rows = components
        .iter()
        .map(|c| c.missing().count())
        .sum::<usize>();
    let short = components
        .iter()
        .filter(|c| c.missing().next().is_some())
        .count();
    let mut parts = Vec::new();
    if rows > 0 {
        parts.push(format!(
            "{rows} preview {} of {short} {} {} no shot in {} capture {}",
            if rows == 1 { "row" } else { "rows" },
            if short == 1 {
                "component"
            } else {
                "components"
            },
            if rows == 1 { "has" } else { "have" },
            if short == 1 { "its" } else { "their" },
            if short == 1 { "file" } else { "files" },
        ));
    }
    if strays > 0 {
        parts.push(format!(
            "{strays} capture {} no preview",
            if strays == 1 {
                "file has"
            } else {
                "files have"
            }
        ));
    }
    if unnamed > 0 {
        parts.push(format!(
            "{unnamed} preview {} no component name",
            if unnamed == 1 {
                "folder has"
            } else {
                "folders have"
            }
        ));
    }
    let gaps = match parts.split_last() {
        None => "Each preview row has its shot in its component's capture file.".to_string(),
        Some((last, [])) => format!("{last}."),
        Some((last, rest)) => format!("{}, and {last}.", rest.join(", ")),
    };
    let mut left_out = Vec::new();
    if design_only > 0 {
        left_out.push(format!(
            "{design_only} {} draw for the design only and have no shots.",
            if design_only == 1 {
                "component"
            } else {
                "components"
            }
        ));
    }
    if design_only_rows > 0 {
        left_out.push(format!(
            "{design_only_rows} preview {} for the design only and {} no shot.",
            if design_only_rows == 1 {
                "row draws"
            } else {
                "rows draw"
            },
            if design_only_rows == 1 { "has" } else { "have" },
        ));
    }
    if left_out.is_empty() {
        return gaps;
    }
    format!("{gaps} {}", left_out.join(" "))
}

#[cfg(test)]
#[path = "component_files_tests.rs"]
mod tests;
