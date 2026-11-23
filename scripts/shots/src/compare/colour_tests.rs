use super::*;

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.001,
        "{actual} is not {expected}"
    );
}

#[test]
fn srgb_converts_to_lab() {
    let white = Lab::of([255, 255, 255]);
    close(white.l, 100.0);
    assert!(white.a.abs() < 0.01 && white.b.abs() < 0.01);
    close(Lab::of([0, 0, 0]).l, 0.0);
    let red = Lab::of([255, 0, 0]);
    assert!((red.l - 53.24).abs() < 0.01, "{red:?}");
    assert!((red.a - 80.09).abs() < 0.02, "{red:?}");
    assert!((red.b - 67.20).abs() < 0.02, "{red:?}");
}

#[test]
fn ciede2000_matches_the_published_pairs() {
    // Sharma, Wu and Dalal, "The CIEDE2000 color-difference formula", pairs 1, 2, 3, 7, 17.
    let pairs = [
        ((50.0, 2.6772, -79.7751), (50.0, 0.0, -82.7485), 2.0425),
        ((50.0, 3.1571, -77.2803), (50.0, 0.0, -82.7485), 2.8615),
        ((50.0, 2.8361, -74.0200), (50.0, 0.0, -82.7485), 3.4412),
        ((50.0, -1.3802, -84.2814), (50.0, 0.0, -82.7485), 1.0000),
        (
            (60.2574, -34.0099, 36.2677),
            (60.4626, -34.1751, 39.4387),
            1.2644,
        ),
    ];
    for ((l1, a1, b1), (l2, a2, b2), expected) in pairs {
        let got = delta_e(
            Lab {
                l: l1,
                a: a1,
                b: b1,
            },
            Lab {
                l: l2,
                a: a2,
                b: b2,
            },
        );
        assert!((got - expected).abs() < 0.0001, "{got} is not {expected}");
    }
}

#[test]
fn equal_colours_do_not_differ() {
    close(rgb_delta([12, 34, 56], [12, 34, 56]), 0.0);
}

fn picture(width: u32, height: u32, fill: impl Fn(u32, u32) -> Rgb) -> Picture {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = fill(x, y);
            pixels.extend([r, g, b, 255]);
        }
    }
    Picture {
        width,
        height,
        pixels,
    }
}

#[test]
fn the_background_is_the_most_common_colour_and_the_foreground_the_most_frequent_far_from_it() {
    let dark = [20, 20, 30];
    let white = [250, 250, 250];
    let near = [22, 20, 30];
    // 100 px box: 70 dark, 6 near-dark, 14 white, 10 grey-ish scattered.
    let picture = picture(10, 10, |x, y| match (y, x) {
        (0..=6, _) => dark,
        (7, 0..=5) => near,
        (7, _) => white,
        (8, 0..=5) => white,
        (8, _) => white,
        _ => [(x * 20) as u8 + 60, 120, 120],
    });
    let bounds = Bounds {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };
    let palette = palette(&picture, 1.0, bounds).unwrap();
    assert_eq!(palette.background, dark);
    assert_eq!(palette.mode().map(|(colour, _)| colour), Some(white));
}

#[test]
fn a_background_of_many_near_shades_beats_a_text_of_one_fill() {
    let text = [168, 168, 179];
    // 10 x 10 box: the text's one fill covers 30 px. The background is a glow of
    // 70 px in 70 near shades, none of which covers more than 1 px.
    let picture = picture(10, 10, |x, y| {
        if y < 3 {
            text
        } else {
            [
                20 + (x % 3) as u8,
                19 + (y % 3) as u8,
                30 + ((x + y) % 7) as u8,
            ]
        }
    });
    let bounds = Bounds {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
    };
    let palette = palette(&picture, 1.0, bounds).unwrap();
    assert!(
        rgb_delta(palette.background, [21, 20, 33]) < 6.0,
        "{:?}",
        palette.background
    );
    assert_eq!(palette.glyph_colour(), Some(text));
}

#[test]
fn a_box_of_one_colour_has_no_foreground() {
    let picture = picture(4, 4, |_, _| [9, 9, 9]);
    let bounds = Bounds {
        x: 0.0,
        y: 0.0,
        width: 4.0,
        height: 4.0,
    };
    let palette = palette(&picture, 1.0, bounds).unwrap();
    assert_eq!(palette.background, [9, 9, 9]);
    assert_eq!(palette.glyph_colour(), None);
}

#[test]
fn a_box_is_read_in_pixels_at_the_scale() {
    // The left half of a 8x4 picture is red; the box is points at scale 2.
    let picture = picture(8, 4, |x, _| if x < 4 { [255, 0, 0] } else { [0, 0, 255] });
    let bounds = Bounds {
        x: 0.0,
        y: 0.0,
        width: 2.0,
        height: 2.0,
    };
    assert_eq!(
        palette(&picture, 2.0, bounds).unwrap().background,
        [255, 0, 0]
    );
}

#[test]
fn hex_formats_colours() {
    assert_eq!(hex([10, 20, 30]), "#0A141E");
}

fn side<'a>(elements: &'a [crate::shots::Element], picture: &'a Picture) -> Side<'a> {
    Side {
        elements,
        picture,
        scale: 1.0,
        unshot: &[],
    }
}

fn labelled(role: Role, words: &str, colour: Option<&str>) -> crate::shots::Element {
    let mut e = crate::compare::normalize::tests::element(role, Some(words), None);
    e.bounds = Bounds {
        x: 0.0,
        y: 0.0,
        width: 8.0,
        height: 8.0,
    };
    e.text = colour.map(|c| crate::shots::TextStyle {
        size: None,
        weight: None,
        color: Some(c.to_string()),
    });
    e
}

fn warn(role: Role, board: &Picture, shot: &Picture) -> Vec<String> {
    let (b, s) = (
        [labelled(role, "Word", None)],
        [labelled(role, "Word", None)],
    );
    let found = crate::compare::compare(&b, &s, crate::compare::Slots::NONE);
    pairs(&side(&b, board), &side(&s, shot), &found.pairs)
        .iter()
        .map(Warning::sentence)
        .collect()
}

#[test]
fn a_background_past_three_warns_and_one_within_it_does_not() {
    let base = picture(8, 8, |_, _| [40, 40, 48]);
    let near = picture(8, 8, |_, _| [41, 40, 48]);
    let far = picture(8, 8, |_, _| [60, 40, 48]);
    assert!(warn(Role::Text, &base, &near).is_empty());
    let found = warn(Role::Text, &base, &far);
    assert_eq!(found.len(), 1);
    assert!(
        found[0].starts_with(
            "The background of the text “Word” is #282830 on the board and #3C2830 in the shot"
        ),
        "{found:?}"
    );
}

#[test]
fn a_foreground_is_compared_where_both_pictures_have_one() {
    let draw = |ink: Rgb| picture(8, 8, move |x, _| if x < 3 { ink } else { [40, 40, 48] });
    let found = warn(Role::Button, &draw([240, 240, 240]), &draw([200, 240, 240]));
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(
        found[0],
        "The foreground of the button “Word” has #F0F0F0 on the board and no colour near it in the shot"
    );
    assert_eq!(
        found[1],
        "The foreground of the button “Word” has #C8F0F0 in the shot and no colour near it on the board"
    );
    let flat = picture(8, 8, |_, _| [40, 40, 48]);
    assert_eq!(
        warn(Role::Button, &draw([240, 240, 240]), &flat),
        [
            "The foreground of the button “Word” has #F0F0F0 on the board and no colour near it in the shot"
        ]
    );
    assert!(warn(Role::Button, &flat, &flat).is_empty());
}

/// An 8 x 8 picture of a button: a label of one colour and an edge of another
/// beside a background, each covering the columns it is given.
fn button(label: Rgb, label_columns: u32, edge: Rgb) -> Picture {
    picture(8, 8, move |x, _| {
        if x < label_columns {
            label
        } else if x < 6 {
            [40, 40, 48]
        } else {
            edge
        }
    })
}

#[test]
fn an_element_that_draws_the_same_two_colours_with_another_one_most_common_does_not_warn() {
    let (label, edge) = ([240, 240, 242], [168, 168, 179]);
    // The label and the edge cover the same area on the board, and the edge
    // covers more than the label in the shot.
    let board = button(label, 2, edge);
    let shot = picture(8, 8, |x, _| match x {
        0 => label,
        1..=2 => edge,
        _ => [40, 40, 48],
    });
    assert!(warn(Role::Button, &board, &shot).is_empty());
    assert!(warn(Role::Toggle, &board, &shot).is_empty());
}

#[test]
fn a_label_drawn_in_the_wrong_colour_warns_on_each_side() {
    let edge = [168, 168, 179];
    let board = button([240, 240, 242], 2, edge);
    let shot = button([238, 152, 193], 2, edge);
    assert_eq!(
        warn(Role::Button, &board, &shot),
        [
            "The foreground of the button “Word” has #F0F0F2 on the board and no colour near it in the shot",
            "The foreground of the button “Word” has #EE98C1 in the shot and no colour near it on the board",
        ]
    );
}

#[test]
fn a_part_matches_the_same_colour_on_the_other_side_at_any_share() {
    let (label, edge) = ([240, 240, 242], [168, 168, 179]);
    // The edge is a part on the board (2 columns of 8) and covers 1 of 64
    // pixels in the shot, which is under the share of a part.
    let board = button(label, 2, edge);
    let shot = picture(8, 8, |x, y| match (x, y) {
        (0..=1, _) => label,
        (7, 0) => edge,
        _ => [40, 40, 48],
    });
    assert!(warn(Role::Button, &board, &shot).is_empty());
}

#[test]
fn a_colour_that_covers_too_little_of_the_box_to_be_a_drawn_part_is_ignored() {
    let label = [240, 240, 242];
    let board = picture(8, 8, |x, _| if x < 3 { label } else { [40, 40, 48] });
    // One antialiasing pixel of another colour: 1 of 64.
    let shot = picture(8, 8, |x, y| match (x, y) {
        (1, 1) => [238, 152, 193],
        (0..=2, _) => label,
        _ => [40, 40, 48],
    });
    assert!(warn(Role::Button, &board, &shot).is_empty());
    assert!(warn(Role::Button, &shot, &board).is_empty());
}

#[test]
fn different_glyph_colours_warn() {
    let draw = |ink: Rgb| picture(8, 8, move |x, _| if x < 3 { ink } else { [40, 40, 48] });
    let found = warn(Role::Text, &draw([255, 255, 255]), &draw([224, 224, 224]));
    assert_eq!(found.len(), 1);
    assert!(
        found[0].starts_with("The colour of the text “Word” is #FFFFFF on the board and #E0E0E0")
    );
}

#[test]
fn rendered_glyphs_of_the_same_colour_do_not_warn() {
    // The board and Mac shot of “Enter a name.”, cropped to the text bounds.
    // The Mac's accessibility style gives a lighter colour than the glyphs.
    let board =
        Picture::decode(include_bytes!("../../tests/fixtures/error-text-board.png")).unwrap();
    let shot = Picture::decode(include_bytes!("../../tests/fixtures/error-text-shot.png")).unwrap();
    let drawn = |picture: &Picture, colour| {
        let mut element = labelled(Role::Text, "Enter a name.", Some(colour));
        element.bounds.width = f64::from(picture.width);
        element.bounds.height = f64::from(picture.height);
        [element]
    };
    let b = drawn(&board, "#F26877FF");
    let s = drawn(&shot, "#F7808AFF");
    let found = crate::compare::compare(&b, &s, crate::compare::Slots::NONE);
    let warnings = pairs(&side(&b, &board), &side(&s, &shot), &found.pairs);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn glyphs_keep_their_colour_when_edge_pixels_outnumber_stem_pixels() {
    for (background, ink, edge) in [
        ([40, 40, 48], [240, 240, 240], [140, 140, 144]),
        ([240, 240, 240], [20, 20, 28], [130, 130, 134]),
    ] {
        let draw = |edge| {
            picture(8, 8, |x, _| match x {
                0 | 2 => edge,
                1 => ink,
                _ => background,
            })
        };
        let board = draw(ink);
        let shot = draw(edge);
        let warnings = warn(Role::Text, &board, &shot);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

#[test]
fn text_ignores_a_contrasting_control_inside_its_bounds() {
    let ink = [155, 155, 162];
    let background = [34, 34, 48];
    let board = picture(8, 8, |x, _| match x {
        0..=2 => ink,
        3 => [238, 152, 193],
        _ => background,
    });
    let shot = picture(8, 8, |x, _| if x <= 2 { ink } else { background });
    let warnings = warn(Role::Text, &board, &shot);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn text_warns_when_only_one_picture_has_measurable_glyph_colour() {
    let background = [40, 40, 48];
    let visible = picture(
        8,
        8,
        |x, _| if x < 3 { [240, 240, 240] } else { background },
    );
    let flat = picture(8, 8, |_, _| background);
    assert_eq!(
        warn(Role::Text, &visible, &flat),
        ["The checker found no measurable glyph colour for the text “Word” in the shot."]
    );
    assert_eq!(
        warn(Role::Text, &flat, &visible),
        ["The checker found no measurable glyph colour for the text “Word” on the board."]
    );
    assert!(warn(Role::Text, &flat, &flat).is_empty());
}
