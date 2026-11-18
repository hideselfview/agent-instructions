//! A design system's components as its Design System page shows them: each
//! component's folder under `design/system/components` holds a
//! `preview.html`, whose first line (`<!-- @dsCard group="..." height=N -->`)
//! gives the height of the card the page draws it in. A preview page is a
//! bare script that draws with React and the design system's components, so
//! whoever draws one loads the design system and React ahead of it, right
//! after its `<head>` (`with_loads`).

use std::path::Path;

use crate::source::{Source, exists_in, in_scratch};
use crate::{Context, Result, fail};

/// The folder of the design system's components.
pub const COMPONENTS: &str = "design/system/components";

/// A component's card: its name, and the card's height or why the card has
/// none.
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
/// `COMPONENTS` that holds a `preview.html`. A commit's folder is read from
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

/// The cards of the components in <top>, a folder that holds `COMPONENTS`
/// at its path in the repo.
fn cards_in(top: &Path) -> Result<Vec<Card>> {
    let folder = top.join(COMPONENTS);
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => fail!("listing {}: {error}", folder.display()),
    };
    let mut cards = Vec::new();
    for entry in entries {
        let entry = entry.context(|| format!("listing {}", folder.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let preview = entry.path().join("preview.html");
        if !is_component_name(&name) || !preview.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&preview)
            .context(|| format!("reading {}", preview_path(&name)))?;
        let height = card_height(text.lines().next().unwrap_or("")).ok_or_else(|| {
            format!(
                "The first line of {} gives its card no height, as <!-- @dsCard height=N --> does.",
                preview_path(&name)
            )
        });
        cards.push(Card { name, height });
    }
    cards.sort_by(|a, b| a.name.cmp(&b.name));
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
