use super::*;

fn name(text: &str) -> Name {
    Name::parse(text).unwrap()
}

#[test]
fn a_mac_window_compares_content_inside_its_corners() {
    let pair = compare_window(
        "mac.welcome.a",
        window_picture(1, 16, [255, 255, 255, 255], false),
        window_picture(2, 16, [0, 0, 0, 0], false),
    );
    assert!(pair.warnings.is_empty(), "{:?}", pair.warnings);
}

#[test]
fn a_different_window_shape_reports_one_warning() {
    for (board, shot) in [(24, 16), (16, 24)] {
        let pair = compare_window_cut(
            "mac.welcome.a",
            f64::from(board),
            window_picture(2, board, [255, 255, 255, 255], false),
            window_picture(2, shot, [0, 0, 0, 0], false),
        );
        assert_eq!(pair.warnings, [Warning::WindowCorners]);
    }
}

#[test]
fn a_board_whose_window_fill_matches_its_backdrop_compares_by_its_window_element() {
    let backdrop = [40, 40, 40, 255];
    let same = compare_window(
        "mac.welcome.a",
        window_picture(2, 16, backdrop, false),
        window_picture(2, 16, [0, 0, 0, 0], false),
    );
    assert!(same.warnings.is_empty(), "{:?}", same.warnings);
    let other = compare_window_cut(
        "mac.welcome.a",
        24.0,
        window_picture(2, 24, backdrop, false),
        window_picture(2, 16, [0, 0, 0, 0], false),
    );
    assert_eq!(other.warnings, [Warning::WindowCorners]);
}

#[test]
fn a_board_whose_page_draws_no_window_element_has_square_corners() {
    let pair = compare_window_cut(
        "mac.welcome.a",
        0.0,
        window_picture(2, 0, [255, 255, 255, 255], false),
        window_picture(2, 16, [0, 0, 0, 0], false),
    );
    assert_eq!(pair.warnings, [Warning::WindowCorners]);
}

#[test]
fn a_board_with_no_window_read_has_no_corners_to_compare() {
    let pair = compare_with_window(
        "mac.welcome.a",
        None,
        window_picture(2, 0, [255, 255, 255, 255], false),
        window_picture(2, 16, [0, 0, 0, 0], false),
    );
    // The pictures still differ at the corners, which the region check marks.
    assert!(!pair.warnings.contains(&Warning::WindowCorners));
    assert_eq!(pair.warnings.len(), 4, "{:?}", pair.warnings);
}

#[test]
fn a_one_pixel_shift_in_the_window_outline_draws_no_warning() {
    let pair = compare_window_cut(
        "mac.welcome.a",
        18.0,
        window_picture(1, 18, [255, 255, 255, 255], false),
        window_picture(1, 16, [0, 0, 0, 0], false),
    );
    assert!(pair.warnings.is_empty(), "{:?}", pair.warnings);
}

#[test]
fn a_difference_inside_a_mac_window_corner_still_marks_its_region() {
    let pair = compare_window(
        "mac.welcome.a",
        window_picture(2, 16, [255, 255, 255, 255], true),
        window_picture(2, 16, [0, 0, 0, 0], false),
    );
    let [Warning::Region { bounds, .. }] = &pair.warnings[..] else {
        panic!("expected one picture region, found {:?}", pair.warnings);
    };
    assert_eq!((bounds.x, bounds.y), (0.0, 0.0));
}

#[test]
fn an_android_picture_compares_its_corners() {
    let pair = compare_window(
        "android.welcome.a",
        window_picture(1, 16, [255, 255, 255, 255], false),
        window_picture(1, 16, [0, 0, 0, 0], false),
    );
    assert_eq!(pair.warnings.len(), 4);
    assert!(
        pair.warnings
            .iter()
            .all(|w| matches!(w, Warning::Region { .. }))
    );
}

#[test]
fn window_buttons_at_the_same_places_compare_equal_whatever_they_are_drawn_as() {
    let pair = compare_window(
        "mac.welcome.a",
        with_buttons(
            window_picture(2, 16, [255, 255, 255, 255], false),
            2,
            &BOARD_BUTTONS,
        ),
        with_buttons(window_picture(2, 16, [0, 0, 0, 0], false), 2, &SHOT_BUTTONS),
    );
    assert!(pair.warnings.is_empty(), "{:?}", pair.warnings);
}

#[test]
fn a_window_button_in_another_place_marks_the_area() {
    let mut moved = SHOT_BUTTONS;
    moved[1].0 += 4.0;
    let pair = compare_window(
        "mac.welcome.a",
        with_buttons(
            window_picture(2, 16, [255, 255, 255, 255], false),
            2,
            &BOARD_BUTTONS,
        ),
        with_buttons(window_picture(2, 16, [0, 0, 0, 0], false), 2, &moved),
    );
    assert!(
        matches!(&pair.warnings[..], [Warning::Region { .. }, ..]),
        "{:?}",
        pair.warnings
    );
}

#[test]
fn a_missing_window_button_marks_the_area() {
    let pair = compare_window(
        "mac.welcome.a",
        with_buttons(
            window_picture(2, 16, [255, 255, 255, 255], false),
            2,
            &BOARD_BUTTONS,
        ),
        with_buttons(
            window_picture(2, 16, [0, 0, 0, 0], false),
            2,
            &SHOT_BUTTONS[..2],
        ),
    );
    assert!(
        matches!(&pair.warnings[..], [Warning::Region { .. }, ..]),
        "{:?}",
        pair.warnings
    );
}

#[test]
fn a_difference_beside_the_window_buttons_still_marks_its_region() {
    let mut shot = with_buttons(window_picture(2, 16, [0, 0, 0, 0], false), 2, &SHOT_BUTTONS);
    paint_disc(&mut shot, 2, (56.0, 30.0), 5.0, [255, 255, 255]);
    let pair = compare_window(
        "mac.welcome.a",
        with_buttons(
            window_picture(2, 16, [255, 255, 255, 255], false),
            2,
            &BOARD_BUTTONS,
        ),
        shot,
    );
    assert!(
        matches!(&pair.warnings[..], [Warning::Region { .. }]),
        "{:?}",
        pair.warnings
    );
}

/// Where a 10 pt button's middle sits, in points, and its colour.
type Button = (f64, f64, [u8; 3]);
const BOARD_BUTTONS: [Button; 3] = [
    (17.0, 20.0, [255, 95, 87]),
    (31.0, 20.0, [254, 188, 46]),
    (45.0, 20.0, [40, 200, 64]),
];
/// The system's buttons: the same places, with a shaded look.
const SHOT_BUTTONS: [Button; 3] = [
    (17.0, 20.0, [160, 20, 20]),
    (31.0, 20.0, [200, 120, 0]),
    (45.0, 20.0, [0, 110, 0]),
];

fn with_buttons(mut picture: Picture, scale: u32, buttons: &[Button]) -> Picture {
    for (x, y, colour) in buttons {
        paint_disc(&mut picture, scale, (*x, *y), 5.0, *colour);
    }
    picture
}

fn paint_disc(picture: &mut Picture, scale: u32, middle: (f64, f64), radius: f64, colour: [u8; 3]) {
    let scale = f64::from(scale);
    for y in 0..picture.height {
        for x in 0..picture.width {
            let (px, py) = ((f64::from(x) + 0.5) / scale, (f64::from(y) + 0.5) / scale);
            if (px - middle.0).hypot(py - middle.1) <= radius {
                let at = (y * picture.width + x) as usize * 4;
                picture.pixels[at..at + 3].copy_from_slice(&colour);
            }
        }
    }
}

/// Compares a board whose window cuts its corners 16 pt with a shot.
fn compare_window(scene: &str, board: Picture, shot: Picture) -> Checked {
    compare_window_cut(scene, 16.0, board, shot)
}

/// Compares a board whose window element cuts each corner `cut` pt straight
/// with a shot.
fn compare_window_cut(scene: &str, cut: f64, board: Picture, shot: Picture) -> Checked {
    compare_with_window(scene, Some(cut), board, shot)
}

/// Compares a board whose list holds a window with the corner cut `cut` (none
/// when no one read the window) with a shot.
fn compare_with_window(scene: &str, cut: Option<f64>, board: Picture, shot: Picture) -> Checked {
    let scene = name(scene);
    let size = crate::shots::Size {
        width: 64.0,
        height: 64.0,
    };
    let list = |width, window| ElementList {
        board: scene.to_string(),
        platform: scene.platform(),
        size,
        scale: f64::from(width) / size.width,
        elements: Vec::new(),
        slots: Vec::new(),
        unshot: Vec::new(),
        window,
    };
    let window = cut.map(|radius| crate::shots::WindowShape {
        frame: crate::shots::Bounds {
            x: 0.0,
            y: 0.0,
            width: size.width,
            height: size.height,
        },
        radius,
        exponent: 1.0,
    });
    compare_pair(
        &scene,
        Drawn {
            list: list(board.width, window),
            picture: board,
            png: PathBuf::new(),
        },
        &Shot {
            list: list(shot.width, None),
            picture: shot,
        },
        PathBuf::new(),
    )
}

/// A flat 64 pt picture with four diagonal cuts and an optional white mark.
fn window_picture(scale: u32, cut: u32, outside: [u8; 4], mark: bool) -> Picture {
    let width = 64 * scale;
    let mut pixels = Vec::new();
    for y in 0..width {
        for x in 0..width {
            let corner_x = x.min(width - 1 - x);
            let corner_y = y.min(width - 1 - y);
            pixels.extend(if corner_x + corner_y < cut * scale {
                outside
            } else if mark
                && (9 * scale..15 * scale).contains(&x)
                && (9 * scale..15 * scale).contains(&y)
            {
                [255, 255, 255, 255]
            } else {
                [40, 40, 40, 255]
            });
        }
    }
    Picture {
        width,
        height: width,
        pixels,
    }
}

fn outcome(checked: Vec<Checked>) -> Outcome {
    Outcome {
        runtime: String::new(),
        checked,
        problems: Vec::new(),
        lists: Lists::default(),
        not_taken: Vec::new(),
        failed: Vec::new(),
        components: crate::component_files::Listing::default(),
    }
}

#[test]
fn a_run_passes_without_findings_or_problems() {
    let mut outcome = outcome(Vec::new());
    outcome.components = crate::component_files::Listing {
        components: Vec::new(),
        strays: vec![crate::component_files::Stray {
            file: "app/Shots/Components/MacGone.swift".into(),
            stem: "MacGone".to_string(),
            why: "no preview".to_string(),
        }],
        unnamed: Vec::new(),
        design_only: 0,
        design_only_rows: 0,
    };
    assert!(
        outcome.passed(),
        "a capture file with no preview fails nothing"
    );
    outcome.problems.push(Problem {
        name: "x".into(),
        sentence: "s".into(),
    });
    assert!(!outcome.passed());
}

/// A pair with one finding, standing <against> the before run.
fn pair_with_finding(against: Option<Against>) -> Checked {
    let finding = Finding::Missing(crate::compare::normalize::Entry {
        index: 0,
        folded: Vec::new(),
        kind: crate::shots::Role::Text,
        words: "x".into(),
        value: String::new(),
        enabled: None,
        selected: false,
    });
    let list = crate::shots::ElementList::parse(crate::shots::tests::LIST).unwrap();
    Checked {
        name: name("mac.welcome.a"),
        board: list.clone(),
        shot: list,
        pairs: Vec::new(),
        offset: (0.0, 0.0),
        board_png: PathBuf::new(),
        shot_png: PathBuf::new(),
        findings: vec![finding],
        warnings: Vec::new(),
        against,
    }
}

#[test]
fn a_pair_with_a_finding_fails_a_run_with_no_before_run() {
    let outcome = outcome(vec![pair_with_finding(None)]);
    assert!(!outcome.passed());
    assert_eq!(outcome.failing().count(), 1);
}

#[test]
fn a_worse_pair_with_a_finding_fails_the_run() {
    let outcome = outcome(vec![pair_with_finding(Some(Against::Worse))]);
    assert!(!outcome.passed());
    assert_eq!(outcome.failing().count(), 1);
}

#[test]
fn a_pair_the_base_did_not_check_lists_its_findings_and_fails_nothing() {
    let outcome = outcome(vec![pair_with_finding(Some(Against::New))]);
    assert!(
        outcome.passed(),
        "a scene with no run to compare with shows its findings for the first time"
    );
    assert_eq!(outcome.failing().count(), 0);
    assert_eq!(outcome.listed().count(), 1);
}

#[test]
fn a_new_pair_still_fails_the_run_when_a_shot_has_no_board_or_a_capture_failed() {
    let mut without_board = outcome(vec![pair_with_finding(Some(Against::New))]);
    without_board.problems.push(Problem {
        name: "mac.new.scene".to_string(),
        sentence: "its shot has no board".to_string(),
    });
    assert!(!without_board.passed());
    let mut failed = outcome(vec![pair_with_finding(Some(Against::New))]);
    failed.failed.push(Failed {
        name: name("mac.new.scene"),
        why: "the capture wrote no picture".to_string(),
    });
    assert!(!failed.passed());
}

#[test]
fn a_pair_that_holds_only_the_before_runs_findings_is_only_listed() {
    let outcome = outcome(vec![pair_with_finding(Some(Against::Held))]);
    assert!(outcome.passed());
    assert_eq!(outcome.listed().count(), 1);
}

#[test]
fn a_new_pair_without_findings_passes() {
    let mut pair = pair_with_finding(Some(Against::New));
    pair.findings.clear();
    assert!(outcome(vec![pair]).passed());
}

#[test]
fn a_pair_the_base_never_rendered_lists_its_findings_and_fails_nothing() {
    let mut without = pair_with_finding(Some(Against::Unrendered));
    without.findings.clear();
    let outcome = outcome(vec![pair_with_finding(Some(Against::Unrendered)), without]);
    assert!(outcome.passed());
    assert_eq!(outcome.failing().count(), 0);
    assert_eq!(outcome.listed().count(), 1);
    assert_eq!(outcome.unrendered().count(), 2);
}

#[test]
fn the_run_of_the_merge_base_lists_every_finding_and_fails_no_pair() {
    let outcome = outcome(vec![
        pair_with_finding(Some(Against::Base)),
        pair_with_finding(Some(Against::Base)),
    ]);
    assert!(
        outcome.passed(),
        "a run with no base fails no pair for lack of one"
    );
    assert_eq!(outcome.failing().count(), 0);
    assert_eq!(outcome.listed().count(), 2);
    assert_eq!(mark(Some(Against::Base)), " (the merge base's own run)");
    let without = pair_with_finding(None);
    assert!(
        !outcome_of(without).passed(),
        "a run with no base and no --base fails each pair with a finding"
    );
}

fn outcome_of(pair: Checked) -> Outcome {
    outcome(vec![pair])
}

#[test]
fn a_problem_fails_a_run_whose_pairs_hold() {
    let mut outcome = outcome(vec![pair_with_finding(Some(Against::Held))]);
    outcome.problems.push(Problem {
        name: "x".into(),
        sentence: "s".into(),
    });
    assert!(!outcome.passed());
}

#[test]
fn a_line_names_how_its_pair_stands_against_the_base() {
    assert_eq!(mark(Some(Against::New)), " (new against the base)");
    assert_eq!(mark(Some(Against::Worse)), " (worse than the base)");
    assert_eq!(
        mark(Some(Against::Unrendered)),
        " (the base never rendered it)"
    );
    assert_eq!(mark(Some(Against::Held)), "");
    assert_eq!(mark(None), "");
}

#[test]
fn the_run_draws_only_the_boards_the_picture_folder_lacks() {
    let dir = tempfile::tempdir().unwrap();
    let names = [name("mac.a.b"), name("mac.a.c")];
    let hashes: BTreeMap<Name, String> = [
        (names[0].clone(), "kept".to_string()),
        (names[1].clone(), "new".to_string()),
    ]
    .into_iter()
    .collect();
    let boards = dir.path().join("boards");
    assert_eq!(
        take_pictures(None, &hashes, &names, &boards).unwrap(),
        names.to_vec()
    );
    let drawn = dir.path().join("drawn");
    std::fs::create_dir_all(&drawn).unwrap();
    std::fs::write(drawn.join("mac.a.b.png"), "png").unwrap();
    std::fs::write(drawn.join("mac.a.b.json"), "{}").unwrap();
    let pictures = Pictures::new(dir.path().join("pictures"), std::time::SystemTime::now());
    pictures.keep_board("kept", "mac.a.b", &drawn).unwrap();
    assert_eq!(
        take_pictures(Some(&pictures), &hashes, &names, &boards).unwrap(),
        [names[1].clone()]
    );
    assert!(boards.join("mac.a.b.png").is_file() && boards.join("mac.a.b.json").is_file());
}
