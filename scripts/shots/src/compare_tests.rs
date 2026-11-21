use super::*;
use crate::compare::normalize::tests::element;
use crate::shots::{Picture, Role};

fn text(words: &str) -> Element {
    element(Role::Text, Some(words), None)
}

fn button(words: &str) -> Element {
    element(Role::Button, Some(words), None)
}

fn sentences(board: &[Element], shot: &[Element]) -> Vec<String> {
    compare(board, shot, &[])
        .findings
        .iter()
        .map(Finding::sentence)
        .collect()
}

#[test]
fn equal_lists_have_no_findings() {
    let list = [text("Title"), button("Set up")];
    assert!(compare(&list, &list, &[]).findings.is_empty());
}

#[test]
fn a_board_element_the_shot_lacks_is_missing() {
    assert_eq!(
        sentences(&[text("Title"), button("Set up")], &[text("Title")]),
        ["Shot is missing the button “Set up”"]
    );
}

#[test]
fn a_shot_element_the_board_lacks_is_extra() {
    assert_eq!(
        sentences(&[text("Title")], &[text("Title"), text("Beta")]),
        ["Shot has the text “Beta”, which the board does not"]
    );
}

fn dialog() -> Element {
    let mut dialog = element(Role::Other, None, None);
    dialog.native = "alertdialog".to_string();
    dialog
}

#[test]
fn a_dialog_the_shot_lists_first_is_not_out_of_order() {
    let board = [
        text("Title"),
        button("Play"),
        dialog(),
        text("Delete"),
        button("OK"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, mut e)| {
        if i > 2 {
            e.parent = Some(2);
        }
        e
    })
    .collect::<Vec<_>>();
    let shot = [text("Delete"), button("OK"), text("Title"), button("Play")];
    assert!(compare(&board, &shot, &[]).findings.is_empty());
}

#[test]
fn a_dialog_read_in_another_order_inside_itself_is_out_of_order() {
    let board = [text("Title"), dialog(), text("Delete"), button("OK")]
        .into_iter()
        .enumerate()
        .map(|(i, mut e)| {
            if i > 1 {
                e.parent = Some(1);
            }
            e
        })
        .collect::<Vec<_>>();
    let shot = [button("OK"), text("Delete"), text("Title")];
    assert_eq!(compare(&board, &shot, &[]).findings.len(), 1);
}

#[test]
fn a_swapped_pair_is_one_out_of_order_finding_not_two() {
    let result = compare(
        &[text("A"), text("B"), text("C")],
        &[text("A"), text("C"), text("B")],
        &[],
    );
    assert_eq!(result.findings.len(), 1);
    assert!(matches!(result.findings[0], Finding::OutOfOrder { .. }));
    assert_eq!(
        result.findings[0].sentence(),
        "The text “B” is in another place in the shot's reading order"
    );
}

#[test]
fn the_same_words_in_another_kind_at_the_same_place_is_a_role_difference() {
    let result = compare(
        &[text("Title"), button("Set up"), text("End")],
        &[text("Title"), text("Set up"), text("End")],
        &[],
    );
    assert_eq!(result.findings.len(), 1);
    assert_eq!(
        result.findings[0].sentence(),
        "“Set up” is a button on the board and a text in the shot"
    );
}

#[test]
fn the_same_words_in_another_kind_far_apart_are_missing_and_extra() {
    let result = compare(
        &[button("Go"), text("A"), text("B"), text("C")],
        &[text("A"), text("B"), text("C"), text("Go")],
        &[],
    );
    let kinds: Vec<_> = result.findings.iter().map(Finding::sentence).collect();
    assert_eq!(
        kinds,
        [
            "Shot is missing the button “Go”",
            "Shot has the text “Go”, which the board does not"
        ]
    );
}

#[test]
fn a_matched_pair_whose_state_differs_says_which_part() {
    let mut on = element(Role::Toggle, Some("Sync"), None);
    on.value = Some("on".to_string());
    let mut off = on.clone();
    off.value = Some("off".to_string());
    assert_eq!(
        sentences(&[on.clone()], &[off]),
        ["The toggle “Sync” has the value “on” on the board and “off” in the shot"]
    );

    let mut disabled = button("Go");
    disabled.enabled = false;
    assert_eq!(
        sentences(&[button("Go")], &[disabled]),
        ["The button “Go” is enabled on the board and disabled in the shot"]
    );

    let mut selected = element(Role::Tab, Some("Library"), None);
    selected.selected = Some(true);
    assert_eq!(
        sentences(&[selected], &[element(Role::Tab, Some("Library"), None)]),
        ["The tab “Library” is selected on the board and not selected in the shot"]
    );
}

#[test]
fn a_field_with_no_text_on_one_side_and_an_empty_string_on_the_other_agrees() {
    let mut empty = element(Role::Field, Some("Name"), None);
    empty.value = Some(String::new());
    let absent = element(Role::Field, Some("Name"), None);
    assert!(compare(&[empty], &[absent], &[]).findings.is_empty());
}

#[test]
fn findings_come_in_reading_order() {
    let result = compare(
        &[text("A"), button("B"), text("C"), button("D")],
        &[text("A"), text("C"), button("D"), text("E")],
        &[],
    );
    let sentences: Vec<_> = result.findings.iter().map(Finding::sentence).collect();
    assert_eq!(
        sentences,
        [
            "Shot is missing the button “B”",
            "Shot has the text “E”, which the board does not"
        ]
    );
}

#[test]
fn an_element_with_no_words_is_described_as_unlabelled() {
    let toggle = element(Role::Toggle, None, None);
    assert_eq!(
        sentences(&[toggle], &[]),
        ["Shot is missing the unlabelled toggle"]
    );
}

fn flat(width: u32, height: u32) -> Picture {
    Picture {
        width,
        height,
        pixels: [40, 40, 48, 255].repeat((width * height) as usize),
    }
}

#[test]
fn an_offset_frame_is_reported_once_and_not_per_element() {
    let board: Vec<Element> = (0..4)
        .map(|i| {
            let mut e = button(&format!("Button {i}"));
            e.bounds = crate::shots::Bounds {
                x: 10.0,
                y: 10.0 + 40.0 * f64::from(i),
                width: 80.0,
                height: 30.0,
            };
            e
        })
        .collect();
    let shot: Vec<Element> = board
        .iter()
        .map(|e| {
            let mut e = e.clone();
            e.bounds.x += 5.0;
            e.bounds.y += 1.0;
            e
        })
        .collect();
    let picture = flat(120, 200);
    let found = compare(&board, &shot, &[]);
    let warnings = warnings(
        &Side {
            elements: &board,
            picture: &picture,
            scale: 1.0,
            unshot: &[],
        },
        &Side {
            elements: &shot,
            picture: &picture,
            scale: 1.0,
            unshot: &[],
        },
        &found.pairs,
    );
    assert_eq!(
        warnings.iter().map(Warning::sentence).collect::<Vec<_>>(),
        ["The shot's frame is 5 pt right and 1 pt below of the board's"]
    );
}

#[test]
fn an_offset_under_the_tolerance_is_not_reported() {
    let mut board = button("Go");
    board.bounds = crate::shots::Bounds {
        x: 10.0,
        y: 10.0,
        width: 80.0,
        height: 30.0,
    };
    let mut shot = board.clone();
    shot.bounds.x += 2.0;
    let picture = flat(120, 60);
    let (board, shot) = ([board], [shot]);
    let found = compare(&board, &shot, &[]);
    assert!(
        warnings(
            &Side {
                elements: &board,
                picture: &picture,
                scale: 1.0,
                unshot: &[]
            },
            &Side {
                elements: &shot,
                picture: &picture,
                scale: 1.0,
                unshot: &[]
            },
            &found.pairs,
        )
        .is_empty()
    );
}

fn field(words: &str) -> Element {
    element(Role::Field, Some(words), None)
}

#[test]
fn a_field_named_with_the_text_under_it_is_the_board_field_and_that_text() {
    let board = [
        text("Title"),
        field("Name"),
        text("Everyone sees this."),
        button("Save"),
    ];
    let shot = [
        text("Title"),
        field("Name Everyone sees this."),
        button("Save"),
    ];
    assert!(compare(&board, &shot, &[]).findings.is_empty());
}

#[test]
fn the_label_text_above_a_field_is_the_field_when_the_shot_draws_it_inside() {
    let board = [
        text("Title"),
        text("Name"),
        field("Name"),
        text("Everyone sees this."),
        button("Save"),
    ];
    let shot = [
        text("Title"),
        field("Name Everyone sees this."),
        button("Save"),
    ];
    assert!(compare(&board, &shot, &[]).findings.is_empty());
}

#[test]
fn the_label_text_above_a_field_stays_a_text_when_the_shot_draws_it_beside() {
    let board = [text("Name"), field("Name")];
    let shot = [text("Name"), field("Name")];
    let found = compare(&board, &shot, &[]);
    assert!(found.findings.is_empty());
    assert_eq!(found.pairs.len(), 2);
}

fn toggle(words: &str) -> Element {
    element(Role::Toggle, Some(words), None)
}

#[test]
fn a_label_and_its_supporting_text_before_a_toggle_are_the_toggle_the_shot_words_as_one() {
    let board = [
        text("Restore"),
        text("Restores the last session."),
        toggle("Restore"),
    ];
    let shot = [toggle("Restore Restores the last session.")];
    assert!(compare(&board, &shot, &[]).findings.is_empty());
}

#[test]
fn a_label_before_a_toggle_is_the_toggle_when_the_shot_has_no_text_of_it() {
    assert!(
        compare(&[text("Pause"), toggle("Pause")], &[toggle("Pause")], &[])
            .findings
            .is_empty()
    );
}

#[test]
fn a_text_that_names_no_field_after_it_is_not_a_label() {
    let board = [text("Name"), button("Go"), field("Name")];
    let shot = [button("Go"), field("Name")];
    assert_eq!(
        sentences(&board, &shot),
        ["Shot is missing the text “Name”"]
    );
}

#[test]
fn a_field_name_that_adds_other_words_is_not_folded() {
    let board = [field("Name"), text("Everyone sees this.")];
    let shot = [field("Name Someone sees this.")];
    assert_eq!(
        sentences(&board, &shot),
        [
            "Shot is missing the field “Name”",
            "Shot is missing the text “Everyone sees this.”",
            "Shot has the field “Name Someone sees this.”, which the board does not"
        ]
    );
}

#[test]
fn the_text_under_a_field_is_not_folded_across_another_element() {
    let board = [field("Name"), button("Go"), text("Everyone sees this.")];
    let shot = [field("Name Everyone sees this."), button("Go")];
    assert!(!compare(&board, &shot, &[]).findings.is_empty());
}

#[test]
fn a_run_of_texts_the_shot_merges_in_order_is_that_one_text() {
    let board = [text("Title"), text("Artist"), text("3:41"), button("Play")];
    let shot = [text("Title Artist 3:41"), button("Play")];
    assert!(compare(&board, &shot, &[]).findings.is_empty());
}

#[test]
fn texts_the_shot_merges_in_another_order_are_not_folded() {
    let board = [text("Title"), text("Artist")];
    let shot = [text("Artist Title")];
    assert_eq!(compare(&board, &shot, &[]).findings.len(), 3);
}

#[test]
fn texts_are_not_folded_across_another_element() {
    let board = [text("Title"), button("Play"), text("Artist")];
    let shot = [text("Title Artist"), button("Play")];
    assert!(!compare(&board, &shot, &[]).findings.is_empty());
}

#[test]
fn a_merged_text_is_not_matched_where_the_shot_keeps_the_texts_apart() {
    let board = [text("Title"), text("Artist")];
    let shot = [text("Title"), text("Artist")];
    let result = compare(&board, &shot, &[]);
    assert!(result.findings.is_empty());
    assert_eq!(result.pairs.len(), 2);
}

fn boxed(mut element: Element, x: f64, y: f64, width: f64, height: f64) -> Element {
    element.bounds = crate::shots::Bounds {
        x,
        y,
        width,
        height,
    };
    element
}

#[test]
fn what_either_side_draws_inside_a_preview_slot_is_left_out() {
    let slot = crate::shots::Bounds {
        x: 10.0,
        y: 100.0,
        width: 200.0,
        height: 80.0,
    };
    let board = [
        boxed(text("Container"), 10.0, 10.0, 80.0, 20.0),
        boxed(text("The section's content"), 20.0, 110.0, 120.0, 20.0),
        boxed(button("Done"), 10.0, 200.0, 60.0, 30.0),
    ];
    // The shot's frame sits 4 points down; its real content fills the slot.
    let shot = [
        boxed(text("Container"), 10.0, 14.0, 80.0, 20.0),
        boxed(button("Open"), 20.0, 114.0, 50.0, 30.0),
        boxed(text("Albums"), 20.0, 150.0, 50.0, 20.0),
        boxed(button("Done"), 10.0, 204.0, 60.0, 30.0),
    ];
    assert_eq!(compare(&board, &shot, &[]).findings.len(), 3);
    let found = compare(&board, &shot, &[slot]);
    assert!(found.findings.is_empty(), "{:?}", found.findings);
    assert_eq!(found.pairs.len(), 2);
}

#[test]
fn a_difference_outside_a_preview_slot_is_still_found() {
    let slot = crate::shots::Bounds {
        x: 10.0,
        y: 100.0,
        width: 200.0,
        height: 80.0,
    };
    let board = [
        boxed(text("Container"), 10.0, 10.0, 80.0, 20.0),
        boxed(text("Stand-in"), 20.0, 110.0, 60.0, 20.0),
    ];
    let shot = [boxed(text("Other"), 10.0, 10.0, 80.0, 20.0)];
    let found = compare(&board, &shot, &[slot]);
    assert_eq!(found.findings.len(), 2);
}

#[test]
fn a_shot_element_that_runs_past_a_preview_slot_is_left_out() {
    // The slot reaches the frame's bottom edge; the real pane scrolls on
    // below it, and a button straddles the slot's top edge.
    let slot = crate::shots::Bounds {
        x: 100.0,
        y: 50.0,
        width: 300.0,
        height: 1.0e9,
    };
    let board = [boxed(text("Container"), 10.0, 10.0, 80.0, 20.0)];
    let shot = [
        boxed(text("Container"), 10.0, 10.0, 80.0, 20.0),
        boxed(button("Reveal"), 120.0, 44.0, 40.0, 20.0),
        boxed(text("Track"), 120.0, 900.0, 40.0, 20.0),
    ];
    let found = compare(&board, &shot, &[slot]);
    assert!(found.findings.is_empty(), "{:?}", found.findings);
}
