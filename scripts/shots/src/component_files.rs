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
use crate::names::{ComponentKey, Name, Platform};
use crate::settings::Settings;
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
    pub(crate) why: String,
}

impl Listing {
    /// The sentence that counts every row the components leave out of their
    /// capture files and every capture file with no preview.
    pub(crate) fn counts(&self) -> String {
        counts(
            &self.components.iter().collect::<Vec<_>>(),
            self.strays.len(),
        )
    }

    /// Whether a row is left out of its capture file or a capture file has
    /// no preview.
    pub(crate) fn has_gaps(&self) -> bool {
        !self.strays.is_empty() || self.components.iter().any(|c| c.missing().next().is_some())
    }
}

impl Stray {
    /// The file's name without its extension, the component it is named for.
    fn stem(&self) -> &str {
        self.file.file_stem().and_then(|s| s.to_str()).unwrap_or("")
    }
}

impl Component {
    /// The rows whose shot the capture file does not declare.
    fn missing(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.place != Place::File)
    }
}

/// The listing for the repo at <root>, with its settings and the shots its
/// harnesses declare. The design system's components are read as
/// `boards::preview::cards` reads them, with <cache> for its scratch folders.
pub(crate) fn list(
    root: &Path,
    cache: &Path,
    settings: &Settings,
    declared: &[Declared],
) -> Result<Listing, String> {
    let platforms: Vec<(Platform, &str, PathBuf)> = settings
        .declared
        .iter()
        .filter_map(|(platform, folder)| {
            declared::source_extension(*platform)
                .map(|extension| (*platform, extension, Path::new(folder).join(COMPONENTS)))
        })
        .collect();
    let cards =
        boards::preview::cards(&boards::Source::Dir(root), cache).map_err(|e| e.to_string())?;
    let previews: Vec<(String, ComponentKey)> = cards
        .into_iter()
        .filter_map(|card| ComponentKey::of(&card.name).map(|key| (card.name, key)))
        .collect();
    let relative = |path: &Path| path.strip_prefix(root).unwrap_or(path).to_path_buf();

    let mut components = Vec::new();
    for (name, key) in &previews {
        let preview = root.join(boards::preview::preview_path(name));
        let html = std::fs::read_to_string(&preview)
            .map_err(|e| format!("cannot read {}: {e}", preview.display()))?;
        let states = crate::lint::rows(&html, &settings.preview_calls);
        for (platform, extension, folder) in &platforms {
            if key.platform.is_some_and(|p| p != *platform) {
                continue;
            }
            let file = folder.join(format!("{name}.{extension}"));
            let file_exists = root.join(&file).is_file();
            let in_file: Vec<Name> = if file_exists {
                declared::declared_in(*platform, &root.join(&file))
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|d| d.name)
                    .collect()
            } else {
                Vec::new()
            };
            let mut rows = Vec::new();
            for state in &states {
                let shot = key
                    .shot(*platform, state)
                    .map_err(|e| format!("{}: {e}", preview.display()))?;
                let place = if in_file.contains(&shot) {
                    Place::File
                } else {
                    match declared.iter().find(|d| d.name == shot) {
                        Some(d) => Place::Elsewhere(relative(&d.file)),
                        None => Place::Nowhere,
                    }
                };
                rows.push(Row { shot, place });
            }
            components.push(Component {
                name: name.clone(),
                platform: *platform,
                file,
                file_exists,
                rows,
            });
        }
    }
    components.sort_by(|a, b| a.name.cmp(&b.name).then(a.platform.cmp(&b.platform)));

    let mut strays = Vec::new();
    for (platform, extension, folder) in &platforms {
        let entries = match std::fs::read_dir(root.join(folder)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("cannot list {}: {e}", folder.display())),
        };
        for entry in entries {
            let path = entry
                .map_err(|e| format!("cannot list {}: {e}", folder.display()))?
                .path();
            if path.extension().and_then(|e| e.to_str()) != Some(*extension) {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let why = match previews.iter().find(|(name, _)| name == stem) {
                Some((_, key)) => match key.platform {
                    Some(owner) if owner != *platform => format!(
                        "{stem} is a component of {}, not of {}",
                        owner.word(),
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
                file: relative(&path),
                why,
            });
        }
    }
    strays.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(Listing { components, strays })
}

/// Fails, naming it, when a name in <only> names no component with a preview
/// and no capture file.
pub(crate) fn check_names(listing: &Listing, only: &[String]) -> Result<(), String> {
    for name in only {
        if !listing.components.iter().any(|c| &c.name == name)
            && !listing.strays.iter().any(|s| s.stem() == name)
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
/// row's place, and the capture files named for them. The last line counts
/// what the lines show.
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
        .filter(|s| only.is_empty() || named(s.stem()))
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
    lines.push(counts(&components, strays.len()));
    lines
}

/// The sentence that counts the rows <components> leave out of their capture
/// files and the <strays>.
fn counts(components: &[&Component], strays: usize) -> String {
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
    if parts.is_empty() {
        "Each preview row has its shot in its component's capture file.".to_string()
    } else {
        format!("{}.", parts.join(", and "))
    }
}

#[cfg(test)]
#[path = "component_files_tests.rs"]
mod tests;
