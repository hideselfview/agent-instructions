//! A design system's components as its Design System page shows them: each
//! component's folder under `design/system/components` holds a
//! `preview.html`, whose first line (`<!-- @dsCard group="..." height=N -->`)
//! gives the height of the card the page draws it in. A preview page is a
//! bare script that draws with React and the design system's components, so
//! whoever draws one loads the design system and React ahead of it, right
//! after its `<head>` (`with_loads`).
//!
//! `design/system/groups.json` lists the components by what they are for:
//! the groups in order, each with a line that introduces it, the components
//! it holds and its subgroups, each with a line and components of its own
//! (`contents`).

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::source::{Source, exists_in, in_scratch};
use crate::{Context, Result, fail};

/// The folder of the design system's components.
pub const COMPONENTS: &str = "design/system/components";

/// The file that lists the design system's components by group.
pub const GROUPS: &str = "design/system/groups.json";

/// A group of `GROUPS` as the file holds it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListedGroup {
    name: String,
    about: String,
    #[serde(default)]
    components: Vec<String>,
    #[serde(default)]
    subgroups: Vec<ListedSubgroup>,
}

/// A subgroup of a group of `GROUPS` as the file holds it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListedSubgroup {
    name: String,
    about: String,
    components: Vec<String>,
}

/// A group of the design system's components: its name, the line that
/// introduces it, the cards of the components it holds itself, and its
/// subgroups.
#[derive(Debug, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub about: String,
    pub cards: Vec<Card>,
    pub subgroups: Vec<Subgroup>,
}

/// A subgroup of a group: its name, the line that introduces it, and the
/// cards of its components.
#[derive(Debug, PartialEq, Eq)]
pub struct Subgroup {
    pub name: String,
    pub about: String,
    pub cards: Vec<Card>,
}

/// The design system's components by group, in the order `GROUPS` lists
/// them, and each way `GROUPS` and the components' folders disagree, as a
/// sentence: a component that no group lists, or that two list; a name
/// that no component's folder has; a group or subgroup with no components.
#[derive(Debug, PartialEq, Eq)]
pub struct Contents {
    pub groups: Vec<Group>,
    pub problems: Vec<String>,
}

/// A component's card: its folder's name, and the card's height or why the
/// card has none.
#[derive(Debug, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    pub height: std::result::Result<u64, String>,
}

/// The path in the repo of component <name>'s preview page.
pub fn preview_path(name: &str) -> String {
    format!("{COMPONENTS}/{name}/preview.html")
}

/// Is <name> a component's name: letters and digits, as a folder of
/// `COMPONENTS` that a page can be asked for?
pub fn is_component_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The cards of the components of <from>, by name: each folder of
/// `COMPONENTS` that holds a `preview.html`. A folder whose name is no
/// component name gets a card of that problem. A commit's folder is read from
/// an export into a scratch folder in <cache>. A source with no `COMPONENTS`
/// has no cards.
pub fn cards(from: &Source, cache: &Path) -> Result<Vec<Card>> {
    match from {
        Source::Dir(dir) => cards_in(dir),
        Source::Commit { repo, rev } => {
            if !exists_in(repo, rev, COMPONENTS)? {
                return Ok(Vec::new());
            }
            in_scratch(cache, |work| {
                from.copy_out(&[COMPONENTS.to_string()], work)?;
                cards_in(work)
            })
        }
    }
}

/// The components of <from> by group, as `GROUPS` lists them. A source
/// with no components needs no `GROUPS`; one with components fails when its
/// `GROUPS` is not there or cannot be read. A commit's components are read
/// as `cards` reads them, in <cache>.
pub fn contents(from: &Source, cache: &Path) -> Result<Contents> {
    let cards = cards(from, cache)?;
    if cards.is_empty() {
        return Ok(Contents {
            groups: Vec::new(),
            problems: Vec::new(),
        });
    }
    let text = from.read(GROUPS, "the list of the design system's groups")?;
    let listed: Vec<ListedGroup> = serde_json::from_str(&text)
        .map_err(|error| crate::Error::new(format!("{}: {error}", from.label(GROUPS))))?;
    Ok(place(listed, cards))
}

/// <cards> in the groups and subgroups of <listed>, with each problem.
fn place(listed: Vec<ListedGroup>, cards: Vec<Card>) -> Contents {
    let mut left: BTreeMap<String, Card> = cards
        .into_iter()
        .map(|card| (card.name.clone(), card))
        .collect();
    let mut problems = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut take = |names: Vec<String>, place: &str, problems: &mut Vec<String>| -> Vec<Card> {
        if names.is_empty() {
            problems.push(format!("{GROUPS} lists no components in {place}."));
        }
        let mut taken = Vec::new();
        for name in names {
            if let Some(first) = seen.get(&name) {
                problems.push(format!(
                    "{GROUPS} lists {name} in {first} and again in {place}."
                ));
            } else if let Some(card) = left.remove(&name) {
                seen.insert(name, place.to_string());
                taken.push(card);
            } else {
                problems.push(format!(
                    "{GROUPS} lists {name} in {place}, and no folder of {COMPONENTS} holds a preview of that name."
                ));
            }
        }
        taken
    };
    let mut groups = Vec::new();
    for group in listed {
        let at = format!("the group {}", group.name);
        let own = if group.components.is_empty() && !group.subgroups.is_empty() {
            Vec::new()
        } else {
            take(group.components, &at, &mut problems)
        };
        let subgroups = group
            .subgroups
            .into_iter()
            .map(|sub| Subgroup {
                cards: take(
                    sub.components,
                    &format!("the subgroup {} of {}", sub.name, group.name),
                    &mut problems,
                ),
                name: sub.name,
                about: sub.about,
            })
            .collect();
        groups.push(Group {
            name: group.name,
            about: group.about,
            cards: own,
            subgroups,
        });
    }
    if !left.is_empty() {
        let names: Vec<&str> = left.keys().map(String::as_str).collect();
        problems.push(format!("{GROUPS} lists no group for {}.", names.join(", ")));
    }
    Contents { groups, problems }
}

/// The names of the folders of `COMPONENTS` that hold a `preview.html`, in
/// <top>, a folder that holds `COMPONENTS` at its path in the repo, in name
/// order, whether or not each name is a component name. A <top> with no
/// `COMPONENTS` has none; a folder whose name is not UTF-8 fails.
pub fn preview_folders(top: &Path) -> Result<Vec<String>> {
    let folder = top.join(COMPONENTS);
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", folder.display()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", folder.display()))?;
        let preview = entry.path().join("preview.html");
        match std::fs::metadata(&preview) {
            Ok(found) if found.is_file() => {}
            Ok(_) => continue,
            // A file of the folder, such as a types file, holds no preview.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(error) => fail!("reading {}: {error}", preview.display()),
        }
        let name = entry.file_name().into_string().map_err(|name| {
            crate::Error::new(format!(
                "{}: the name {name:?} of a folder that holds a preview.html is not UTF-8",
                folder.display()
            ))
        })?;
        names.push(name);
    }
    names.sort();
    Ok(names)
}

/// The cards of the components in <top>, a folder that holds `COMPONENTS`
/// at its path in the repo.
fn cards_in(top: &Path) -> Result<Vec<Card>> {
    let mut cards = Vec::new();
    for name in preview_folders(top)? {
        let preview = top.join(preview_path(&name));
        let height = if is_component_name(&name) {
            let text = std::fs::read_to_string(&preview)
                .context(|| format!("reading {}", preview_path(&name)))?;
            card_height(text.lines().next().unwrap_or("")).ok_or_else(|| {
                format!(
                    "The first line of {} gives its card no height, as <!-- @dsCard height=N --> does.",
                    preview_path(&name)
                )
            })
        } else {
            Err(format!(
                "{COMPONENTS}/{name} holds a preview.html, and its folder's name is no component name: a component's name is letters and digits."
            ))
        };
        cards.push(Card { name, height });
    }
    Ok(cards)
}

/// The height that <line>, a preview's first line, gives its card: the
/// `height=N` of its `@dsCard` comment.
fn card_height(line: &str) -> Option<u64> {
    let marker = line.trim().strip_prefix("<!--")?.trim_start();
    let attributes = marker.strip_prefix("@dsCard")?;
    attributes
        .split_whitespace()
        .find_map(|attribute| attribute.strip_prefix("height="))
        .and_then(|value| value.trim_end_matches("-->").parse().ok())
}

/// <html>, a preview page, with <loads> right after its `<head>` tag, so
/// that the design system and React load before the page's own script;
/// None when the page has no `<head>`.
pub fn with_loads(html: &str, loads: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    let end = loop {
        let start = from + lower[from..].find("<head")?;
        let after = &lower[start + "<head".len()..];
        if after.starts_with('>') || after.starts_with(char::is_whitespace) {
            break start + lower[start..].find('>')? + 1;
        }
        from = start + "<head".len();
    };
    Some(format!("{}{loads}{}", &html[..end], &html[end..]))
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
