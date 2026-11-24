//! Bringing a board's element list and a shot's to the same terms before they
//! are aligned: words as `shots::words` makes them, the text inside a named
//! control folded into the control's name, and elements with no words (which
//! are structure, or pictures the regions check covers) left out.

use crate::shots::{Bounds, Element, Role, words};

/// An element as the comparison sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The element's index in the list it came from.
    pub(crate) index: usize,
    /// The indices of the elements folded into this one by [`fold_merged`]:
    /// the texts after a board field or text that the shot holds as one.
    pub(crate) folded: Vec<usize>,
    pub(crate) kind: Role,
    pub(crate) words: String,
    /// A field's text, `on` or `off`, a pop-up's choice, a slider's number;
    /// empty when none, and for every role whose value the board does not
    /// state (`Role::has_value`), whatever the list says.
    pub(crate) value: String,
    /// Static text has no enabled state of its own.
    pub(crate) enabled: Option<bool>,
    pub(crate) selected: bool,
}

impl Entry {
    /// The box of the element, with the boxes of what was folded into it:
    /// the shot draws all of them as one.
    pub(crate) fn bounds(&self, list: &[Element]) -> Bounds {
        let boxes = std::iter::once(&self.index)
            .chain(&self.folded)
            .map(|i| list[*i].bounds);
        let (mut left, mut top, mut right, mut bottom) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for b in boxes {
            left = left.min(b.x);
            top = top.min(b.y);
            right = right.max(b.x + b.width);
            bottom = bottom.max(b.y + b.height);
        }
        Bounds {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }
    }

    pub(crate) fn key(&self) -> (Role, String) {
        (self.kind, self.words.clone())
    }
}

/// The words an element is known by: its label, or for a field with none the
/// placeholder it shows. SwiftUI joins the texts of a combined control with
/// ", " where a board joins them with a space, so a control's label reads
/// each ", " as a space, on both sides. A text keeps its own commas.
fn name(element: &Element) -> String {
    let label = element.label.as_deref().map(words).unwrap_or_default();
    let label = if element.role == Role::Text {
        label
    } else {
        words(&label.replace(", ", " "))
    };
    match (&element.placeholder, label.is_empty(), element.role) {
        (Some(placeholder), true, Role::Field) => words(placeholder),
        _ => label,
    }
}

/// AppKit can report a parent's disabled state on a static label.
pub(crate) fn enabled_state(kind: Role, enabled: bool) -> Option<bool> {
    (kind != Role::Text).then_some(enabled)
}

/// Whether the text at `index` is the inner half of a selectable text. SwiftUI
/// shows `.textSelection(.enabled)` text to accessibility as a static text
/// that holds one static text with the same label at the same frame. The pair
/// is one text, which the outer element stands for.
fn is_selectable_copy(list: &[Element], index: usize) -> bool {
    let element = &list[index];
    let Some(parent) = element.parent else {
        return false;
    };
    let holder = &list[parent];
    element.role == Role::Text
        && holder.role == Role::Text
        && element.bounds == holder.bounds
        && name(element) == name(holder)
        && list.iter().filter(|e| e.parent == Some(parent)).count() == 1
}

/// The entries of `list`, in its order.
pub(crate) fn entries(list: &[Element]) -> Vec<Entry> {
    // Whether a named control holds the element, directly or further up.
    let mut inside_control = vec![false; list.len()];
    let mut entries = Vec::new();
    for (index, element) in list.iter().enumerate() {
        let words = name(element);
        if let Some(parent) = element.parent.filter(|p| *p < index) {
            let holder = &list[parent];
            inside_control[index] =
                inside_control[parent] || (holder.role.is_control() && !name(holder).is_empty());
        }
        let structure = match element.role {
            Role::Text | Role::Image | Role::Other => words.is_empty(),
            _ => false,
        };
        if structure
            || (element.role == Role::Text && inside_control[index])
            || is_selectable_copy(list, index)
        {
            continue;
        }
        entries.push(Entry {
            index,
            folded: Vec::new(),
            kind: element.role,
            words,
            value: element
                .value
                .as_deref()
                .filter(|_| element.role.has_value())
                .map(crate::shots::words)
                .unwrap_or_default(),
            enabled: enabled_state(element.role, element.enabled),
            selected: element.selected.unwrap_or(false),
        });
    }
    entries
}

/// The board's entries in the shot's terms, where the shot's tree merges what
/// the board keeps apart: a field named with the supporting text under it, and
/// a tile, row or title whose texts Compose merges into one text. A text or
/// field followed by a run of texts whose words, joined in order, are one
/// element of the same kind in the shot is that one element. A run that does
/// not join to something the shot holds, or that has another kind of element
/// inside it, is left as it is, and the comparison reports it.
pub(crate) fn fold_merged(board: Vec<Entry>, shot: &[Entry]) -> Vec<Entry> {
    // The shot's keys not yet taken by a fold.
    let mut available: Vec<_> = shot.iter().map(Entry::key).collect();
    let board = fold_labels(board, &available);
    let mut folded = Vec::with_capacity(board.len());
    let mut at = 0;
    while at < board.len() {
        let head = &board[at];
        let run = if matches!(head.kind, Role::Field | Role::Text) {
            board[at + 1..]
                .iter()
                .take_while(|e| e.kind == Role::Text)
                .count()
        } else {
            0
        };
        let merged = (1..=run).rev().find_map(|tail| {
            let mut parts = vec![head.words.as_str()];
            parts.extend(board[at + 1..=at + tail].iter().map(|e| e.words.as_str()));
            let key = (head.kind, parts.join(" "));
            available
                .iter()
                .position(|k| *k == key)
                .map(|place| (tail, place, key))
        });
        match merged {
            Some((tail, place, key)) => {
                available.swap_remove(place);
                let with: Vec<usize> = board[at + 1..=at + tail].iter().map(|e| e.index).collect();
                let mut all = head.folded.clone();
                all.extend(with);
                folded.push(Entry {
                    words: key.1,
                    folded: all,
                    ..head.clone()
                });
                at += tail + 1;
            }
            None => {
                folded.push(head.clone());
                at += 1;
            }
        }
    }
    folded
}

/// The board's fields and toggles with the label text before each one folded
/// in, where the shot draws the label inside the control: a Compose field or
/// switch row is one element that holds its label, and the supporting text
/// that comes between the label and the control. A label text is a text
/// before a field or toggle that is named with the same words, and any texts
/// between them are its supporting text. When the shot holds the control
/// named with the label and that supporting text, they are that control; when
/// it holds the control by the label alone and no text of the label's words,
/// the label is the control's. A shot that draws the label as a text (the Mac
/// draws it beside the field) keeps the board's text as a text.
fn fold_labels(board: Vec<Entry>, available: &[(Role, String)]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(board.len());
    // The texts just before the entry being looked at, not yet written out.
    let mut texts: Vec<Entry> = Vec::new();
    for entry in board {
        if entry.kind == Role::Text {
            texts.push(entry);
            continue;
        }
        let label = matches!(entry.kind, Role::Field | Role::Toggle)
            .then(|| texts.iter().rposition(|t| t.words == entry.words))
            .flatten();
        match label {
            Some(at) => {
                out.extend(texts.drain(..at));
                out.extend(fold_label(entry, std::mem::take(&mut texts), available));
            }
            None => {
                out.append(&mut texts);
                out.push(entry);
            }
        }
    }
    out.append(&mut texts);
    out
}

/// `control`, the label text that opens `texts` and the supporting texts after
/// it, as the entries the comparison sees: the control with what the shot draws
/// inside it folded in, then the texts that stay.
fn fold_label(
    mut control: Entry,
    mut texts: Vec<Entry>,
    available: &[(Role, String)],
) -> Vec<Entry> {
    let label = texts.remove(0);
    let kind = control.kind;
    let holds = |words: &str| available.iter().any(|k| *k == (kind, words.to_string()));
    let with_support = std::iter::once(label.words.as_str())
        .chain(texts.iter().map(|t| t.words.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    if !texts.is_empty() && holds(&with_support) {
        control.words = with_support;
        control
            .folded
            .extend(std::iter::once(&label).chain(&texts).map(|t| t.index));
        return vec![control];
    }
    if available
        .iter()
        .any(|k| *k == (Role::Text, label.words.clone()))
    {
        let mut out = vec![label];
        out.extend(texts);
        out.push(control);
        return out;
    }
    control.folded.push(label.index);
    texts.push(control);
    texts
}

#[cfg(test)]
#[path = "normalize_tests.rs"]
pub(crate) mod tests;
