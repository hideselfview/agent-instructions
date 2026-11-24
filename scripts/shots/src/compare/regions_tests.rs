use super::*;
use crate::compare::normalize::tests::element;
use crate::shots::Role;

const GREY: [u8; 3] = [40, 40, 48];
const WHITE: [u8; 3] = [240, 240, 240];

/// A `width` by `height` pixel picture of grey with white squares at the
/// given pixel rectangles (x, y, size).
fn picture(width: u32, height: u32, squares: &[(u32, u32, u32)]) -> Picture {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let inside = squares
                .iter()
                .any(|(sx, sy, n)| x >= *sx && x < sx + n && y >= *sy && y < sy + n);
            let [r, g, b] = if inside { WHITE } else { GREY };
            pixels.extend([r, g, b, 255]);
        }
    }
    Picture {
        width,
        height,
        pixels,
    }
}

fn side<'a>(elements: &'a [Element], picture: &'a Picture, scale: f64) -> Side<'a> {
    Side {
        elements,
        picture,
        scale,
        unshot: &[],
    }
}

#[test]
fn equal_pictures_have_no_regions() {
    let p = picture(64, 64, &[(20, 20, 8)]);
    assert!(regions(&side(&[], &p, 1.0), &side(&[], &p, 1.0), (0.0, 0.0), None).is_empty());
}

#[test]
fn a_changed_icon_marks_its_cell() {
    let board = picture(64, 64, &[(20, 20, 8)]);
    let shot = picture(64, 64, &[]);
    let found = regions(
        &side(&[], &board, 1.0),
        &side(&[], &shot, 1.0),
        (0.0, 0.0),
        None,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].bounds,
        Bounds {
            x: 16.0,
            y: 16.0,
            width: 16.0,
            height: 16.0
        }
    );
    assert!(found[0].delta > 3.0);
}

#[test]
fn neighbouring_cells_merge_into_one_rectangle() {
    let board = picture(64, 64, &[(10, 20, 10), (30, 20, 10)]);
    let shot = picture(64, 64, &[]);
    let found = regions(
        &side(&[], &board, 1.0),
        &side(&[], &shot, 1.0),
        (0.0, 0.0),
        None,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].bounds,
        Bounds {
            x: 0.0,
            y: 16.0,
            width: 48.0,
            height: 16.0
        }
    );
}

#[test]
fn cells_apart_are_separate_regions() {
    let board = picture(64, 64, &[(2, 2, 8), (50, 50, 8)]);
    let shot = picture(64, 64, &[]);
    let found = regions(
        &side(&[], &board, 1.0),
        &side(&[], &shot, 1.0),
        (0.0, 0.0),
        None,
    );
    assert_eq!(found.len(), 2);
}

#[test]
fn a_difference_inside_a_text_or_control_box_is_left_to_the_other_checks() {
    let board = picture(64, 64, &[(20, 20, 8)]);
    let shot = picture(64, 64, &[]);
    let mut label = element(Role::Button, Some("Go"), None);
    label.bounds = Bounds {
        x: 10.0,
        y: 10.0,
        width: 40.0,
        height: 40.0,
    };
    let list = [label];
    let found = regions(
        &side(&list, &board, 1.0),
        &side(&list, &shot, 1.0),
        (0.0, 0.0),
        None,
    );
    assert!(found.is_empty());
}

#[test]
fn an_image_element_is_compared() {
    let board = picture(64, 64, &[(20, 20, 8)]);
    let shot = picture(64, 64, &[]);
    let mut image = element(Role::Image, Some("Cover"), None);
    image.bounds = Bounds {
        x: 10.0,
        y: 10.0,
        width: 40.0,
        height: 40.0,
    };
    let list = [image];
    let found = regions(
        &side(&list, &board, 1.0),
        &side(&list, &shot, 1.0),
        (0.0, 0.0),
        None,
    );
    assert_eq!(found.len(), 1);
}

#[test]
fn the_frames_are_aligned_by_the_offset_and_pictures_by_their_scale() {
    // The board's square at (20, 20) pt; the shot draws it 4 pt right and
    // 4 pt down at two pixels to the point.
    let board = picture(64, 64, &[(20, 20, 8)]);
    let shot = picture(128, 128, &[(48, 48, 16)]);
    let found = regions(
        &side(&[], &board, 1.0),
        &side(&[], &shot, 2.0),
        (4.0, 4.0),
        None,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_area_the_board_marks_as_not_in_the_shot_is_left_out() {
    let board = picture(64, 64, &[(20, 40, 8)]);
    let shot = picture(64, 64, &[]);
    let area = [Bounds {
        x: 0.0,
        y: 32.0,
        width: 64.0,
        height: 32.0,
    }];
    let mut with = side(&[], &board, 1.0);
    with.unshot = &area;
    assert!(regions(&with, &side(&[], &shot, 1.0), (0.0, 0.0), None).is_empty());
    assert_eq!(
        regions(
            &side(&[], &board, 1.0),
            &side(&[], &shot, 1.0),
            (0.0, 0.0),
            None
        )
        .len(),
        1
    );
}
