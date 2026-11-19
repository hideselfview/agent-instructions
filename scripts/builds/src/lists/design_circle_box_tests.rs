use super::*;
use crate::lists::ReviewBoard;
use crate::lists::design::{ReviewSection, Theme};
use crate::lists::fixtures::{Refusals, db, design_drawn, read, row, v};

/// A design change whose review page holds one board, and its id.
fn one_board(db: &Connection) -> Id {
    design_drawn(
        db,
        "/d/bae-worktrees/side-2",
        &[],
        Some(ReviewSection {
            heading: "What changed".into(),
            words: String::new(),
            boards: vec![ReviewBoard {
                board: "design/mocks/A.dc.html".into(),
                caption: "the A".into(),
                theme: Theme::Dark,
                circles: vec![],
            }],
        }),
    )
}

#[test]
fn every_named_constraint_of_the_box_circles_refuses_its_row() {
    let db = db();
    let id = one_board(&db);
    let mut boxes = Refusals::new(include_str!("design_circle_box.sql"));
    let boxed = [
        ("design_id", v(id.0)),
        ("board", v(0)),
        ("position", v(1)),
        ("x", v(10)),
        ("y", v(20)),
        ("width", v(300)),
        ("height", v(40)),
    ];
    boxes.check(
        "review_circle_box_position_not_negative",
        plant(&db, &row(&boxed, &[("position", v(-1))])),
    );
    boxes.check(
        "review_circle_box_width_is_positive",
        plant(&db, &row(&boxed, &[("width", v(0))])),
    );
    boxes.check(
        "review_circle_box_height_is_positive",
        plant(&db, &row(&boxed, &[("height", v(0))])),
    );
    boxes.check(
        "review_circle_box_label_says_something",
        plant(&db, &row(&boxed, &[("label", v(" "))])),
    );
    boxes.check_as(
        "review_circle_box_is_on_a_board",
        "FOREIGN KEY",
        plant(&db, &row(&boxed, &[("board", v(1))])),
    );
    plant(&db, &boxed).unwrap();
    boxes.done();
}

#[test]
fn a_written_box_circle_reads_back_under_its_board_with_its_position() {
    let db = db();
    let id = one_board(&db);
    let area = BoardBox {
        x: 10,
        y: 20,
        width: 300,
        height: 40,
    };
    write(&db, id, 0, 2, &area, Some("The sheet")).unwrap();
    assert_eq!(
        all(&db).unwrap(),
        BTreeMap::from([(
            (id, 0),
            vec![(
                2,
                Circle {
                    around: Around::Box(area),
                    label: Some("The sheet".into()),
                }
            )]
        )])
    );
    // design::read puts it among the board's circles.
    let lists = read(&db);
    let section = lists.design(id).unwrap().review.as_ref().unwrap();
    assert_eq!(section.boards[0].circles.len(), 1);
}
