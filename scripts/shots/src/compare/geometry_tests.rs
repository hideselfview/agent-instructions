use super::*;
use crate::compare::Slots;
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
    let pairs = compare(board, shot, Slots::NONE).pairs;
    let offset = frame_offset(board, shot, &pairs);
    let mut all = boxes(board, shot, &pairs, offset, 0.0);
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
    let pairs = compare(&board, &shot, Slots::NONE).pairs;
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

#[test]
fn a_text_box_that_spans_its_container_and_is_centred_on_the_board_text_does_not_move() {
    let board = texts();
    let mut shot = board.clone();
    // The shot's box is the whole 52 pt bar the title sits in, centred on the
    // board's 20 pt line, and as wide as the bar.
    shot[0].bounds = Bounds {
        x: 0.0,
        y: -6.0,
        width: 300.0,
        height: 52.0,
    };
    assert!(warnings(&board, &shot).is_empty());
}

#[test]
fn a_text_box_taller_than_the_board_text_and_not_centred_on_it_is_resized() {
    let board = texts();
    let mut shot = board.clone();
    // Taller, with its middle 8 pt below the board's.
    shot[0].bounds.height = 36.0;
    shot[0].bounds.y = 10.0;
    let found: Vec<_> = warnings(&board, &shot)
        .iter()
        .map(Warning::sentence)
        .collect();
    assert_eq!(found, ["“Title” is 16 pt taller in the shot"]);
}

#[test]
fn a_text_box_no_wider_than_the_board_text_compares_its_height() {
    let board = texts();
    let mut shot = board.clone();
    // Centred on the board's line and taller, but not wider: no container.
    shot[0].bounds.y -= 10.0;
    shot[0].bounds.height += 20.0;
    let found: Vec<_> = warnings(&board, &shot)
        .iter()
        .map(Warning::sentence)
        .collect();
    assert_eq!(
        found,
        [
            "“Title” moved 10 pt up in the shot",
            "“Title” is 20 pt taller in the shot"
        ]
    );
}

#[test]
fn a_text_box_of_another_width_says_nothing_of_the_frames_left_edge() {
    // A line of text the board shrink-wraps and the shot lays out at its full
    // width, centred in both: the box's left edge moves 6.3 pt and the text
    // stays where it is. The button beside it fixes the frame at no offset.
    let board = vec![
        boxed(Role::Text, "Scan", 46.3, 305.0, 318.3, 30.0),
        boxed(Role::Button, "Cancel", 160.1, 364.0, 90.9, 40.0),
    ];
    let shot = vec![
        boxed(Role::Text, "Scan", 40.0, 304.9, 331.0, 30.1),
        boxed(Role::Button, "Cancel", 160.0, 364.0, 91.0, 40.0),
    ];
    let pairs = compare(&board, &shot, Slots::NONE).pairs;
    let (dx, dy) = frame_offset(&board, &shot, &pairs);
    assert!(dx.abs() < 0.1, "{dx}");
    assert!(dy.abs() < 0.1, "{dy}");
    assert!(warnings(&board, &shot).is_empty());
}

#[test]
fn texts_of_another_width_give_the_horizontal_offset_when_nothing_else_does() {
    // Only texts pair, and every shot box is wider and starts 4 pt left: the
    // offset is still measured, from their left edges.
    let board = vec![boxed(Role::Text, "Pinned", 32.0, 2.0, 360.0, 36.0)];
    let shot = vec![boxed(Role::Text, "Pinned", 28.0, 1.5, 383.0, 36.0)];
    let pairs = compare(&board, &shot, Slots::NONE).pairs;
    assert_eq!(frame_offset(&board, &shot, &pairs), (-4.0, -0.5));
}

/// The geometry warnings of two lists whose shot rounds each glyph advance to
/// a whole pixel: `slack` points of error for each glyph.
fn warnings_with_slack(board: &[Element], shot: &[Element], slack: f64) -> Vec<String> {
    let pairs = compare(board, shot, Slots::NONE).pairs;
    let offset = frame_offset(board, shot, &pairs);
    boxes(board, shot, &pairs, offset, slack)
        .iter()
        .map(Warning::sentence)
        .collect()
}

#[test]
fn a_text_width_within_the_rounding_of_its_glyph_advances_is_the_same_layout() {
    // 22 glyphs, each advance off by up to a quarter point: a width 4 pt
    // wider is the same line, and its left edge is compared; a width 8 pt
    // wider is another layout, a box that spans its container.
    let board = vec![
        boxed(
            Role::Text,
            "Your music, everywhere",
            20.0,
            10.0,
            160.0,
            20.0,
        ),
        boxed(Role::Button, "Save", 10.0, 200.0, 80.0, 40.0),
        boxed(Role::Button, "Cancel", 100.0, 200.0, 80.0, 40.0),
    ];
    let mut shot = board.clone();
    shot[0].bounds.width += 4.0;
    shot[0].bounds.x += 9.0;
    assert!(warnings_with_slack(&board, &shot, 0.0).is_empty());
    assert_eq!(warnings_with_slack(&board, &shot, 0.25).len(), 1);
    shot[0].bounds.width += 4.0;
    assert!(warnings_with_slack(&board, &shot, 0.25).is_empty());
}

#[test]
fn a_text_after_others_on_its_line_may_sit_off_by_their_rounding_too() {
    // Three texts in a row: each starts after the earlier ones, so the third
    // is off by the sum of their glyph errors: 2.3 pt left, with 20 glyphs
    // before it, 12 of its own and 2 pt of box tolerance.
    let row = |x3: f64| {
        vec![
            boxed(Role::Text, "Harbor Records", 20.0, 10.0, 70.0, 16.0),
            boxed(Role::Text, "Pinned", 100.0, 10.0, 40.0, 16.0),
            boxed(Role::Text, "Catalog link", x3, 10.0, 60.0, 16.0),
        ]
    };
    let board = row(150.0);
    let shot = row(147.7);
    assert_eq!(warnings_with_slack(&board, &shot, 0.0).len(), 1);
    assert!(warnings_with_slack(&board, &shot, 0.25).is_empty());
    // A text alone on its line has only its own glyphs to round.
    let alone = vec![
        boxed(Role::Text, "Pinned", 150.0, 60.0, 40.0, 16.0),
        boxed(Role::Button, "Save", 10.0, 200.0, 80.0, 40.0),
        boxed(Role::Button, "Cancel", 100.0, 200.0, 80.0, 40.0),
    ];
    let mut moved = alone.clone();
    moved[0].bounds.x -= 2.3;
    assert!(warnings_with_slack(&alone, &moved, 0.25).is_empty());
    moved[0].bounds.x -= 3.0;
    assert_eq!(warnings_with_slack(&alone, &moved, 0.25).len(), 1);
}

#[test]
fn a_chip_after_others_on_its_line_may_sit_off_by_their_glyph_rounding() {
    // Chips a label sizes, in a row: the fourth ends 2.6 pt short and starts
    // 1.4 pt short, though no chip is more than a pixel off by itself.
    let row = |x: f64, width: f64| {
        vec![
            boxed(Role::Button, "MusicBrainz ↗", 0.0, 0.0, 84.0, 19.0),
            boxed(Role::Button, "Discogs ↗", 92.0, 0.0, 62.0, 19.0),
            boxed(Role::Button, "Bandcamp ↗", 162.0, 0.0, 75.0, 19.0),
            boxed(
                Role::Button,
                "Harbor Records catalog ↗",
                x,
                0.0,
                width,
                19.0,
            ),
        ]
    };
    let board = row(246.4, 139.7);
    let shot = row(245.0, 138.5);
    assert_eq!(warnings_with_slack(&board, &shot, 0.0).len(), 1);
    assert!(warnings_with_slack(&board, &shot, 0.25).is_empty());
}

/// Three images, the first two symbols in text lines as a board boxes them
/// (the line's height), and two buttons that fix the frame.
fn symbols() -> Vec<Element> {
    vec![
        boxed(Role::Image, "Identified", 100.0, 50.0, 13.0, 16.0),
        boxed(Role::Image, "Queued", 200.0, 50.0, 12.0, 16.0),
        boxed(Role::Image, "Cover", 300.0, 50.0, 40.0, 40.0),
        boxed(Role::Button, "One", 10.0, 10.0, 100.0, 30.0),
        boxed(Role::Button, "Two", 10.0, 90.0, 100.0, 30.0),
    ]
}

/// The box the Mac reports for a symbol: its glyph, at the board box's
/// trailing bottom corner.
fn glyph_in(board: &Element, width: f64, height: f64) -> Bounds {
    let b = board.bounds;
    Bounds {
        x: b.x + b.width - width,
        y: b.y + b.height - height,
        width,
        height,
    }
}

#[test]
fn an_image_drawn_smaller_than_its_box_and_inside_it_matches() {
    let board = symbols();
    let mut shot = board.clone();
    shot[0].bounds = glyph_in(&board[0], 10.0, 10.0);
    shot[1].bounds = glyph_in(&board[1], 11.5, 11.5);
    assert!(warnings(&board, &shot).is_empty());
}

#[test]
fn an_image_drawn_smaller_than_its_box_and_outside_it_moved() {
    let board = symbols();
    let mut shot = board.clone();
    shot[0].bounds = glyph_in(&board[0], 10.0, 10.0);
    shot[0].bounds.y += 8.0;
    let found: Vec<_> = warnings(&board, &shot)
        .iter()
        .map(Warning::sentence)
        .collect();
    assert_eq!(
        found,
        [
            "“Identified” moved 8 pt down in the shot",
            "“Identified” is 3 pt narrower and 6 pt shorter in the shot"
        ]
    );
}

#[test]
fn an_image_drawn_larger_than_its_box_is_resized() {
    let board = symbols();
    let mut shot = board.clone();
    shot[2].bounds.width += 10.0;
    shot[2].bounds.height += 10.0;
    shot[2].bounds.x -= 5.0;
    shot[2].bounds.y -= 5.0;
    let found = warnings(&board, &shot);
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].sentence(),
        "“Cover” is 10 pt wider and 10 pt taller in the shot"
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
    let pairs = compare(&board, &shot, Slots::NONE).pairs;
    assert_eq!(pairs.len(), 1);
    assert!(boxes(&board, &shot, &pairs, (0.0, 0.0), 0.0).is_empty());
    assert_eq!(frame_offset(&board, &shot, &pairs), (0.0, 0.0));
    // A shot text that is taller than the run still warns.
    let tall = vec![boxed(Role::Text, "Title Artist", 10.0, 10.0, 60.0, 50.0)];
    let pairs = compare(&board, &tall, Slots::NONE).pairs;
    assert_eq!(boxes(&board, &tall, &pairs, (0.0, 0.0), 0.0).len(), 1);
}
