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
    let (background, foreground) = palette(&picture, 1.0, bounds).unwrap();
    assert_eq!(background, dark);
    assert_eq!(foreground, Some(white));
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
    assert_eq!(palette(&picture, 1.0, bounds), Some(([9, 9, 9], None)));
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
    assert_eq!(palette(&picture, 2.0, bounds).unwrap().0, [255, 0, 0]);
}

#[test]
fn hex_colours_round_trip_without_alpha() {
    assert_eq!(parse_hex("#FFFFFF80"), Some([255, 255, 255]));
    assert_eq!(parse_hex("#0a141E"), Some([10, 20, 30]));
    assert_eq!(parse_hex("white"), None);
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

fn labelled(words: &str, colour: Option<&str>) -> crate::shots::Element {
    let mut e =
        crate::compare::normalize::tests::element(crate::shots::Role::Text, Some(words), None);
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

fn warn(board: (&Picture, Option<&str>), shot: (&Picture, Option<&str>)) -> Vec<String> {
    let (b, s) = ([labelled("Word", board.1)], [labelled("Word", shot.1)]);
    let found = crate::compare::compare(&b, &s, &[]);
    pairs(&side(&b, board.0), &side(&s, shot.0), &found.pairs)
        .iter()
        .map(Warning::sentence)
        .collect()
}

#[test]
fn a_background_past_three_warns_and_one_within_it_does_not() {
    let base = picture(8, 8, |_, _| [40, 40, 48]);
    let near = picture(8, 8, |_, _| [41, 40, 48]);
    let far = picture(8, 8, |_, _| [60, 40, 48]);
    assert!(warn((&base, None), (&near, None)).is_empty());
    let found = warn((&base, None), (&far, None));
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
    let found = warn(
        (&draw([240, 240, 240]), None),
        (&draw([200, 240, 240]), None),
    );
    assert_eq!(found.len(), 1);
    assert!(found[0].starts_with("The foreground of"), "{found:?}");
    let flat = picture(8, 8, |_, _| [40, 40, 48]);
    assert!(warn((&draw([240, 240, 240]), None), (&flat, None)).is_empty());
}

#[test]
fn a_text_colour_from_the_style_is_compared_where_both_sides_give_it() {
    let flat = picture(8, 8, |_, _| [40, 40, 48]);
    let found = warn((&flat, Some("#FFFFFFFF")), (&flat, Some("#E0E0E0FF")));
    assert_eq!(found.len(), 1);
    assert!(
        found[0].starts_with("The colour of the text “Word” is #FFFFFF on the board and #E0E0E0")
    );
    assert!(warn((&flat, Some("#FFFFFFFF")), (&flat, None)).is_empty());
}
