use super::*;
use crate::compare::compare;
use crate::compare::normalize::tests::element;
use crate::shots::TextStyle;

fn boxed(role: Role, words: &str, x: f64, y: f64, width: f64, height: f64) -> Element {
    let mut e = element(role, Some(words), None);
    e.bounds = Bounds {
        x,
        y,
        width,
        height,
    };
    e
}

/// Three buttons and the pairs a comparison finds between the lists.
fn board() -> Vec<Element> {
    vec![
        boxed(Role::Button, "One", 10.0, 10.0, 100.0, 30.0),
        boxed(Role::Button, "Two", 10.0, 50.0, 100.0, 30.0),
        boxed(Role::Button, "Three", 10.0, 90.0, 100.0, 30.0),
    ]
}

fn warnings(board: &[Element], shot: &[Element]) -> Vec<Warning> {
    let pairs = compare(board, shot).pairs;
    let offset = frame_offset(board, shot, &pairs);
    let mut all = boxes(board, shot, &pairs, offset);
    all.extend(fonts(board, shot, &pairs));
    all
}

fn shifted(list: &[Element], dx: f64, dy: f64) -> Vec<Element> {
    list.iter()
        .map(|e| {
            let mut e = e.clone();
            e.bounds.x += dx;
            e.bounds.y += dy;
            e
        })
        .collect()
}

#[test]
fn an_offset_frame_is_one_offset_and_no_element_moves() {
    let board = board();
    let shot = shifted(&board, 6.0, -3.0);
    let pairs = compare(&board, &shot).pairs;
    assert_eq!(frame_offset(&board, &shot, &pairs), (6.0, -3.0));
    assert!(warnings(&board, &shot).is_empty());
}

#[test]
fn two_points_pass_and_two_and_a_half_warn() {
    let board = board();
    let mut shot = board.clone();
    shot[0].bounds.y += 2.0;
    assert!(warnings(&board, &shot).is_empty());
    shot[0].bounds.y += 0.5;
    let found = warnings(&board, &shot);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].sentence(), "“One” moved 2.5 pt down in the shot");
}

#[test]
fn a_box_that_grew_on_one_side_is_resized_not_moved() {
    let board = board();
    let mut shot = board.clone();
    shot[1].bounds.width += 5.0;
    let found = warnings(&board, &shot);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].sentence(), "“Two” is 5 pt wider in the shot");
}

#[test]
fn a_box_that_grew_about_its_middle_is_only_resized() {
    let board = board();
    let mut shot = board.clone();
    shot[1].bounds.x -= 3.0;
    shot[1].bounds.width += 6.0;
    let found = warnings(&board, &shot);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].sentence(), "“Two” is 6 pt wider in the shot");
}

fn texts() -> Vec<Element> {
    vec![
        boxed(Role::Text, "Title", 20.0, 10.0, 80.0, 20.0),
        boxed(Role::Text, "Body", 20.0, 40.0, 120.0, 40.0),
        boxed(Role::Text, "Note", 20.0, 90.0, 60.0, 16.0),
    ]
}

#[test]
fn a_text_box_spanning_its_container_compares_no_left_edge() {
    let board = texts();
    let mut shot = board.clone();
    // Spans from the container's edge: left and width differ, top and height agree.
    shot[1].bounds.x = 0.0;
    shot[1].bounds.width = 300.0;
    assert!(warnings(&board, &shot).is_empty());
}

#[test]
fn a_text_box_of_the_same_width_compares_its_left_edge() {
    let board = texts();
    let mut shot = board.clone();
    shot[1].bounds.x += 4.0;
    let found = warnings(&board, &shot);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].sentence(), "“Body” moved 4 pt right in the shot");
}

#[test]
fn a_text_compares_its_top_and_height_even_when_it_spans_its_container() {
    let board = texts();
    let mut shot = board.clone();
    shot[1].bounds.width = 300.0;
    shot[1].bounds.y += 3.0;
    shot[1].bounds.height += 4.0;
    let found: Vec<_> = warnings(&board, &shot)
        .iter()
        .map(Warning::sentence)
        .collect();
    assert_eq!(
        found,
        [
            "“Body” moved 3 pt down in the shot",
            "“Body” is 4 pt taller in the shot"
        ]
    );
}

fn styled(size: Option<f64>, weight: Option<u32>) -> Element {
    let mut e = boxed(Role::Text, "Word", 0.0, 0.0, 10.0, 10.0);
    e.text = Some(TextStyle {
        size,
        weight,
        color: None,
    });
    e
}

#[test]
fn font_size_warns_past_half_a_point_and_weight_on_any_difference() {
    let board = [styled(Some(13.0), Some(400))];
    assert!(warnings(&board, &[styled(Some(13.5), Some(400))]).is_empty());
    let found: Vec<_> = warnings(&board, &[styled(Some(13.6), Some(600))])
        .iter()
        .map(Warning::sentence)
        .collect();
    assert_eq!(
        found,
        [
            "“Word” is set at 13.6 pt in the shot and 13 pt on the board",
            "“Word” is set at weight 600 in the shot and 400 on the board"
        ]
    );
}

#[test]
fn a_font_part_only_one_side_gives_is_not_compared() {
    let board = [styled(Some(13.0), None)];
    assert!(warnings(&board, &[styled(None, Some(700))]).is_empty());
}

#[test]
fn a_folded_run_is_compared_by_the_union_of_its_boxes() {
    let board = vec![
        boxed(Role::Text, "Title", 10.0, 10.0, 60.0, 14.0),
        boxed(Role::Text, "Artist", 10.0, 26.0, 40.0, 14.0),
    ];
    let shot = vec![boxed(Role::Text, "Title Artist", 10.0, 10.0, 60.0, 30.0)];
    let pairs = compare(&board, &shot).pairs;
    assert_eq!(pairs.len(), 1);
    assert!(boxes(&board, &shot, &pairs, (0.0, 0.0)).is_empty());
    assert_eq!(frame_offset(&board, &shot, &pairs), (0.0, 0.0));
    // A shot text that is taller than the run still warns.
    let tall = vec![boxed(Role::Text, "Title Artist", 10.0, 10.0, 60.0, 50.0)];
    let pairs = compare(&board, &tall).pairs;
    assert_eq!(boxes(&board, &tall, &pairs, (0.0, 0.0)).len(), 1);
}
