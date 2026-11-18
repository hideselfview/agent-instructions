use super::*;
use crate::shots::{Bounds, Element, Role};

pub(crate) fn element(role: Role, label: Option<&str>, parent: Option<usize>) -> Element {
    Element {
        role,
        native: String::new(),
        label: label.map(str::to_string),
        value: None,
        placeholder: None,
        bounds: Bounds {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
        enabled: true,
        selected: None,
        focused: None,
        text: None,
        parent,
    }
}

#[test]
fn words_are_composed_collapsed_and_trimmed_and_nothing_else() {
    let list = [element(
        Role::Text,
        Some("  Cafe\u{301}\n  “quoted”… "),
        None,
    )];
    let entries = entries(&list);
    assert_eq!(entries[0].words, "Caf\u{e9} “quoted”…");
}

#[test]
fn a_text_inside_a_named_control_is_the_controls_name() {
    let list = [
        element(Role::Button, Some("Set up"), None),
        element(Role::Text, Some("Set up"), Some(0)),
        element(Role::Text, Some("Title"), None),
    ];
    let kept: Vec<_> = entries(&list).iter().map(|e| (e.index, e.kind)).collect();
    assert_eq!(kept, [(0, Role::Button), (2, Role::Text)]);
}

#[test]
fn a_text_inside_a_control_without_a_name_stays() {
    let list = [
        element(Role::Button, None, None),
        element(Role::Text, Some("Go"), Some(0)),
    ];
    let kept: Vec<_> = entries(&list).iter().map(|e| e.index).collect();
    assert_eq!(kept, [0, 1]);
}

#[test]
fn a_text_deeper_inside_a_named_control_folds_too() {
    let list = [
        element(Role::Button, Some("Go"), None),
        element(Role::Other, None, Some(0)),
        element(Role::Text, Some("Go"), Some(1)),
    ];
    assert_eq!(entries(&list).len(), 1);
}

#[test]
fn elements_with_no_words_are_structure() {
    let list = [
        element(Role::Other, None, None),
        element(Role::Other, Some(" "), None),
        element(Role::Text, None, None),
        element(Role::Image, None, None),
        element(Role::Toggle, None, None),
        element(Role::Other, Some("Card"), None),
    ];
    let kept: Vec<_> = entries(&list).iter().map(|e| (e.index, e.kind)).collect();
    assert_eq!(kept, [(4, Role::Toggle), (5, Role::Other)]);
}

#[test]
fn a_field_without_a_label_is_named_by_its_placeholder() {
    let mut field = element(Role::Field, None, None);
    field.placeholder = Some("Restore code".to_string());
    assert_eq!(entries(&[field])[0].words, "Restore code");
}

#[test]
fn state_is_the_value_enabled_and_selected() {
    let mut toggle = element(Role::Toggle, Some("Sync"), None);
    toggle.value = Some(" on ".to_string());
    toggle.enabled = false;
    toggle.selected = Some(true);
    let blank = element(Role::Field, Some("Name"), None);
    let entries = entries(&[toggle, blank]);
    assert_eq!(
        (
            entries[0].value.as_str(),
            entries[0].enabled,
            entries[0].selected
        ),
        ("on", false, true)
    );
    assert_eq!(
        (entries[1].value.as_str(), entries[1].selected),
        ("", false)
    );
}
