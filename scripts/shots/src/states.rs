//! The component states the boards draw that no preview row shows.
//!
//! The checker draws every board of canvas.json and every component's
//! preview page, and reads each design-system component drawn there with
//! the props it was drawn with (render/instances.rs). A prop is part of a
//! component's state when its value is
//!
//! - `true`: a boolean prop that is `false` or unset is one value, unset; or
//! - a code word: a string that starts with a lower-case letter and holds
//!   only letters, digits, `-` and `_` (`primary`, `moveFailed`,
//!   `expand_more`), for a prop the component never draws as words on any
//!   page read.
//!
//! Only an instance that draws a node counts, and only the props its page
//! sets: a component that draws nothing for a state has no row to show, and
//! the props a component gives to a component it draws are the design
//! system's code. A component that returns a bare string draws a text node,
//! which counts as words.
//!
//! The other props are not state: `children`, callbacks and every prop that
//! holds a React element, an array or an object; numbers, which measure;
//! words, the strings a component draws as text, an accessible name, a
//! title, a placeholder, alt text or a field's value, or that are no code
//! word; `className`, which styles an element from outside; and an icon's
//! name (`icon`, `name`, or a prop whose name ends in `Icon`), which picks a
//! picture as words do. Each preview row shows the component with one value changed from
//! the preview's example, so a board's instance shows in the preview when
//! each of its state props has its value, or is unset, on an instance of the
//! component in one of the preview's rows. The props a component's state
//! has are those any instance of it, on a board or in its preview, sets to a
//! state value. A component whose preview draws it in no row shows in no row
//! whatever its props. A component with no preview has no component shot,
//! so the listing leaves it out.

use crate::Checker;
use crate::cli::StatesArgs;
use crate::design;
use crate::pictures::Pictures;
use crate::render;
use crate::render::instances::Instance;
use crate::render::pages::{self, Context, Page, Store};
use futures::StreamExt;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

/// How many pages draw at once.
const PAGES: usize = 8;

/// How many boards a line names before it counts the rest.
const BOARDS_NAMED: usize = 3;

/// Every board's instances and every preview's, as drawn.
#[derive(Debug, Default)]
pub(crate) struct Drawn {
    /// Each board with the components it draws, by board name.
    pub(crate) boards: Vec<(String, Vec<Instance>)>,
    /// Each component with a preview, with the components its preview page
    /// draws, by component name.
    pub(crate) previews: Vec<(String, Vec<Instance>)>,
}

/// The value of one prop of a component's state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Value {
    /// Unset, or `false`.
    Unset,
    /// `true`.
    On,
    /// A code word.
    Word(String),
}

/// What of a component no preview row shows.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Gap {
    /// One value of one prop of its state.
    Value { prop: String, value: Value },
    /// The component itself: its preview draws it in no row.
    NoRow,
}

/// A component state the boards draw that no preview row shows, with the
/// boards that draw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Uncovered {
    pub(crate) component: String,
    pub(crate) gap: Gap,
    /// The boards, by name, each once.
    pub(crate) boards: Vec<String>,
}

impl fmt::Display for Uncovered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.component)?;
        match &self.gap {
            Gap::Value { prop, value } => match value {
                Value::On => write!(f, " {prop}")?,
                Value::Unset => write!(f, " {prop} unset")?,
                Value::Word(word) => write!(f, " {prop} \"{word}\"")?,
            },
            Gap::NoRow => write!(f, ", which its preview draws in no row")?,
        }
        let named = &self.boards[..self.boards.len().min(BOARDS_NAMED)];
        write!(f, ": {}", named.join(", "))?;
        match self.boards.len() - named.len() {
            0 => Ok(()),
            1 => write!(f, " and 1 more board"),
            more => write!(f, " and {more} more boards"),
        }
    }
}

/// Is <text> a code word: a lower-case letter, then letters, digits, `-`
/// and `_`?
fn code_word(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Each instance on a board and in a preview.
fn every_instance(drawn: &Drawn) -> impl Iterator<Item = &Instance> {
    drawn
        .boards
        .iter()
        .chain(&drawn.previews)
        .flat_map(|(_, found)| found)
}

/// Does <instance> count toward the states? It must draw a node, because a
/// preview row of a component that draws nothing is an empty shot, and the
/// page must set its props: the props that a component gives to a component
/// it draws are the design system's code, not a choice a board makes.
fn counts(instance: &Instance) -> bool {
    instance.draws && instance.by_page
}

/// Is <prop> a class name, which styles an element from outside, or an
/// icon's name (`icon`, `name`, or a name that ends in `Icon`), which picks a
/// picture as words do? Neither is a state of the component.
fn styles_or_names(prop: &str) -> bool {
    matches!(prop, "className" | "icon" | "name") || prop.ends_with("Icon")
}

/// The state props an instance sets, with their values: each prop that is
/// `true`, or a code word that no instance of the component draws as words
/// (<words>, by component and prop), other than a class name or an icon's
/// name.
fn state(instance: &Instance, words: &BTreeSet<(&str, &str)>) -> BTreeMap<String, Value> {
    instance
        .props
        .iter()
        .filter(|(prop, _)| !styles_or_names(prop))
        .filter_map(|(prop, value)| {
            let value = match value {
                serde_json::Value::Bool(true) => Value::On,
                serde_json::Value::String(text)
                    if code_word(text)
                        && !words.contains(&(instance.component.as_str(), prop.as_str())) =>
                {
                    Value::Word(text.clone())
                }
                _ => return None,
            };
            Some((prop.clone(), value))
        })
        .collect()
}

/// Every component state the boards draw that no preview row shows, by
/// component and then by what the row would show.
pub(crate) fn uncovered(drawn: &Drawn) -> Vec<Uncovered> {
    let words: BTreeSet<(&str, &str)> = every_instance(drawn)
        .flat_map(|i| i.words.iter().map(|w| (i.component.as_str(), w.as_str())))
        .collect();
    // Each component's preview instances in rows, as states.
    let mut shown: BTreeMap<&str, Vec<BTreeMap<String, Value>>> = BTreeMap::new();
    for (component, found) in &drawn.previews {
        shown.entry(component).or_default().extend(
            found
                .iter()
                .filter(|i| counts(i) && i.in_row && &i.component == component)
                .map(|i| state(i, &words)),
        );
    }
    let boards: Vec<(&str, &Instance, BTreeMap<String, Value>)> = drawn
        .boards
        .iter()
        .flat_map(|(board, found)| found.iter().map(move |i| (board.as_str(), i)))
        .filter(|(_, i)| counts(i))
        .map(|(board, i)| (board, i, state(i, &words)))
        .collect();
    // Each component's state props: those any instance of it sets.
    let mut props: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (_, instance, values) in &boards {
        props
            .entry(&instance.component)
            .or_default()
            .extend(values.keys().map(String::as_str));
    }
    for (component, states) in &shown {
        props
            .entry(component)
            .or_default()
            .extend(states.iter().flat_map(|s| s.keys().map(String::as_str)));
    }
    let mut gaps: BTreeMap<(String, Gap), BTreeSet<&str>> = BTreeMap::new();
    for (board, instance, values) in &boards {
        let component = instance.component.as_str();
        // A component with no preview has no component shot to cover it.
        let Some(rows) = shown.get(component) else {
            continue;
        };
        if rows.is_empty() {
            gaps.entry((component.to_string(), Gap::NoRow))
                .or_default()
                .insert(board);
            continue;
        }
        for prop in &props[component] {
            let value = values.get(*prop).cloned().unwrap_or(Value::Unset);
            let in_a_row = rows
                .iter()
                .any(|row| row.get(*prop).unwrap_or(&Value::Unset) == &value);
            if !in_a_row {
                gaps.entry((
                    component.to_string(),
                    Gap::Value {
                        prop: prop.to_string(),
                        value,
                    },
                ))
                .or_default()
                .insert(board);
            }
        }
    }
    gaps.into_iter()
        .map(|((component, gap), boards)| Uncovered {
            component,
            gap,
            boards: boards.into_iter().map(str::to_string).collect(),
        })
        .collect()
}

/// The states of <now> that <before> does not hold: the states a change
/// adds.
pub(crate) fn added(now: Vec<Uncovered>, before: &[Uncovered]) -> Vec<Uncovered> {
    now.into_iter()
        .filter(|n| {
            !before
                .iter()
                .any(|b| b.component == n.component && b.gap == n.gap)
        })
        .collect()
}

/// The lines that list <uncovered>, each state on its own, and the line that
/// counts them. <against> names the revision the states were found to be
/// new against, when they were.
pub(crate) fn lines(uncovered: &[Uncovered], against: Option<&str>) -> Vec<String> {
    let mut lines: Vec<String> = uncovered.iter().map(ToString::to_string).collect();
    let components = uncovered
        .iter()
        .map(|u| u.component.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let states = match uncovered.len() {
        1 => "1 component state".to_string(),
        n => format!("{n} component states"),
    };
    let of = match components {
        1 => "1 component".to_string(),
        n => format!("{n} components"),
    };
    lines.push(match (uncovered.len(), against) {
        (0, None) => "Every component state that a board draws shows in a preview row.".to_string(),
        (0, Some(rev)) => format!(
            "The boards draw no component state that no preview row shows and the listing at {rev} does not hold."
        ),
        (_, None) => format!("The boards draw {states} of {of} that no preview row shows."),
        (_, Some(rev)) => format!(
            "The boards draw {states} of {of} that no preview row shows and the listing at {rev} does not hold: add each state as a row of its component's preview."
        ),
    });
    lines
}

/// `builds shots states`: prints the component states the boards of the repo
/// at <root> draw that no preview row shows (or, with `--against`, those the
/// listing at that revision lacks); false when `--against` finds one.
pub(crate) fn command(checker: &Checker, root: &Path, args: StatesArgs) -> Result<bool, String> {
    let pictures = args
        .pictures
        .map(|dir| Pictures::new(dir, std::time::SystemTime::now()));
    let store = Store {
        capture_version: &checker.versions.capture,
        pictures: pictures.as_ref(),
    };
    let now = read(root, args.at.as_deref(), checker.folders, &store)?;
    let listed = match &args.against {
        Some(rev) => added(now, &read(root, Some(rev), checker.folders, &store)?),
        None => now,
    };
    for line in lines(&listed, args.against.as_deref()) {
        crate::log::outcome(line);
    }
    Ok(args.against.is_none() || listed.is_empty())
}

/// The component states of the repo at <root> that no preview row shows, as
/// its files are, or as they are at revision <at>, which is written into a
/// scratch folder of the cache of <folders> to be drawn.
pub(crate) fn read(
    root: &Path,
    at: Option<&str>,
    folders: &boards::Folders,
    store: &Store,
) -> Result<Vec<Uncovered>, String> {
    let Some(rev) = at else {
        return Ok(uncovered(&draw(root, folders, store)?));
    };
    design::at_revision(root, rev, &folders.cache, |work| {
        draw(work, folders, store).map(|drawn| uncovered(&drawn))
    })
}

/// The components every board and every preview of the repo at <root> draws:
/// the instances the picture folder keeps for the page's hash, and for each
/// page it lacks, the ones read from a drawing of the page, which it keeps. A
/// repo with no preview has no state to list, so nothing is read.
fn draw(root: &Path, folders: &boards::Folders, store: &Store) -> Result<Drawn, String> {
    let context = Context::of(root, folders)?;
    let Some((first, _)) = context.previews.components.first() else {
        return Ok(Drawn::default());
    };
    // A preview's rows are the ones the settings' marks select, which a repo
    // with a preview must have.
    context.settings.preview(first).map_err(|e| e.to_string())?;
    let pages: Vec<(Page, bool, String)> = context
        .frames
        .iter()
        .map(|(name, frame)| (Page::Board(name, *frame), true))
        .chain(
            context
                .previews
                .components
                .iter()
                .map(|(name, _)| (Page::Preview(name), false)),
        )
        .map(|(page, is_board)| {
            let hash = page.hash(root, &context, store.capture_version)?;
            Ok((page, is_board, hash))
        })
        .collect::<Result<_, String>>()?;
    let mut read: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
    let mut lacking = Vec::new();
    for (page, _, hash) in &pages {
        let kept = match store.pictures {
            Some(pictures) => pictures.take_instances(hash)?,
            None => None,
        };
        match kept {
            Some(text) => {
                let found = serde_json::from_str(&text)
                    .map_err(|e| format!("the instances kept for {hash}: {e}"))?;
                read.insert(hash.clone(), found);
            }
            None => lacking.push((*page, hash.clone())),
        }
    }
    crate::log::note(format_args!(
        "took the components of {} of {} pages from the picture folder; drawing {}",
        pages.len() - lacking.len(),
        pages.len(),
        lacking.len()
    ));
    if !lacking.is_empty() {
        // The components a page draws do not depend on the device scale.
        let found =
            render::with_browser(context.tree(root), folders, 1.0, async |server, chrome| {
                futures::stream::iter(
                    lacking
                        .iter()
                        .map(|(page, _)| pages::read_instances(*page, &context, server, chrome)),
                )
                .buffered(PAGES)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
            })?;
        for ((_, hash), instances) in lacking.iter().zip(found) {
            if let Some(pictures) = store.pictures {
                let text = serde_json::to_string(&instances).map_err(|e| e.to_string())?;
                pictures.keep_instances(hash, &text)?;
            }
            read.insert(hash.clone(), instances);
        }
    }
    let mut drawn = Drawn::default();
    for (page, is_board, hash) in pages {
        let (name, instances) = match page {
            Page::Board(name, _) | Page::Preview(name) => (name, read[&hash].clone()),
        };
        let list = if is_board {
            &mut drawn.boards
        } else {
            &mut drawn.previews
        };
        list.push((name.to_string(), instances));
    }
    drawn.boards.sort_by(|a, b| a.0.cmp(&b.0));
    drawn.previews.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(drawn)
}

#[cfg(test)]
#[path = "states_tests.rs"]
mod tests;
