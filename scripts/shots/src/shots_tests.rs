use super::*;
use std::path::Path;

/// A list in the shape both harnesses write: a field with its placeholder, a
/// button that is not enabled, text with a style, and a nested group.
pub(crate) const LIST: &str = r##"{
  "board": "mac.welcome.a",
  "elements": [
    {
      "bounds": { "height": 20, "width": 61, "x": 16, "y": 16 },
      "enabled": true,
      "focused": null,
      "label": "Enter code",
      "native": "AXStaticText",
      "parent": null,
      "role": "text",
      "selected": null,
      "text": { "color": "#FFFFFFFF", "size": 13, "weight": 600 },
      "value": null
    },
    {
      "bounds": { "height": 22, "width": 348, "x": 26, "y": 60 },
      "enabled": true,
      "focused": true,
      "label": null,
      "native": "AXTextField",
      "parent": null,
      "placeholder": "Restore code or invite",
      "role": "field",
      "selected": null,
      "value": null
    },
    {
      "bounds": { "height": 22, "width": 70, "x": 300, "y": 150 },
      "enabled": false,
      "focused": null,
      "label": "Continue",
      "native": "AXButton",
      "parent": 1,
      "role": "button",
      "selected": null,
      "value": null
    }
  ],
  "platform": "macos",
  "scale": 2,
  "size": { "height": 190, "width": 400 }
}"##;

pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
    png_of(width, height, |_, _| [17, 17, 25, 255])
}

/// The PNG of a picture <width> by <height> pixels whose pixel at (x, y) is
/// <pixel>(x, y).
pub(crate) fn png_of(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// Writes `name`'s PNG, `width` by `height` pixels, and its element list.
pub(crate) fn write_pair(dir: &Path, name: &Name, list: &str, width: u32, height: u32) {
    std::fs::write(dir.join(name.file_name("json")), list).unwrap();
    std::fs::write(dir.join(name.file_name("png")), png(width, height)).unwrap();
}

#[test]
fn a_list_reads_each_key_the_harness_writes() {
    let list = ElementList::parse(LIST).unwrap();
    assert_eq!(list.board, "mac.welcome.a");
    assert_eq!(list.platform, Platform::Mac);
    assert_eq!(
        (list.size.width, list.size.height, list.scale),
        (400.0, 190.0, 2.0)
    );
    let [title, field, button] = &list.elements[..] else {
        panic!("three elements");
    };
    assert_eq!(title.role, Role::Text);
    assert_eq!(title.label.as_deref(), Some("Enter code"));
    let style = title.text.as_ref().unwrap();
    assert_eq!((style.size, style.weight), (Some(13.0), Some(600)));
    assert_eq!(style.color.as_deref(), Some("#FFFFFFFF"));
    assert_eq!(field.role, Role::Field);
    assert_eq!(field.placeholder.as_deref(), Some("Restore code or invite"));
    assert_eq!(field.focused, Some(true));
    assert_eq!(button.parent, Some(1));
    assert!(!button.enabled);
    assert_eq!(button.bounds.x, 300.0);
}

#[test]
fn a_key_the_platform_left_out_stays_absent() {
    let list = ElementList::parse(LIST).unwrap();
    let [title, field, button] = &list.elements[..] else {
        panic!("three elements");
    };
    assert!(title.placeholder.is_none());
    assert!(field.text.is_none());
    assert!(button.text.is_none() && button.placeholder.is_none());
    assert_eq!((title.focused, button.focused), (None, None));
    // Android writes no `focused` key unless the node says; a style may hold one part.
    let android = LIST
        .replace("\"focused\": null,", "")
        .replace("macos", "android")
        .replace(
            r##""color": "#FFFFFFFF", "size": 13, "weight": 600"##,
            r#""size": 14"#,
        );
    let list = ElementList::parse(&android).unwrap();
    assert_eq!(list.platform, Platform::Android);
    assert_eq!(list.elements[0].focused, None);
    let style = list.elements[0].text.as_ref().unwrap();
    assert_eq!(
        (style.size, style.weight, style.color.as_deref()),
        (Some(14.0), None, None)
    );
}

#[test]
fn an_unknown_key_is_refused_at_every_level() {
    for changed in [
        LIST.replacen("\"scale\": 2,", "\"scale\": 2, \"dpi\": 320,", 1),
        LIST.replacen(
            "\"size\": { \"height\"",
            "\"size\": { \"depth\": 1, \"height\"",
            1,
        ),
        LIST.replacen(
            "\"native\": \"AXButton\",",
            "\"native\": \"AXButton\", \"hint\": \"x\",",
            1,
        ),
        LIST.replacen("\"x\": 16,", "\"x\": 16, \"z\": 1,", 1),
        LIST.replacen("\"weight\": 600", "\"weight\": 600, \"italic\": true", 1),
    ] {
        let error = ElementList::parse(&changed).unwrap_err().to_string();
        assert!(error.contains("unknown field"), "{error}");
    }
}

#[test]
fn an_unknown_role_or_platform_is_refused() {
    let role = LIST.replacen("\"role\": \"button\"", "\"role\": \"menu\"", 1);
    assert!(ElementList::parse(&role).is_err());
    let platform = LIST.replacen("\"macos\"", "\"mac\"", 1);
    let error = ElementList::parse(&platform).unwrap_err().to_string();
    assert!(error.contains("no platform"), "{error}");
}

#[test]
fn a_shot_loads_its_list_and_its_picture() {
    let dir = tempfile::tempdir().unwrap();
    let name = Name::parse("mac.welcome.a").unwrap();
    write_pair(dir.path(), &name, LIST, 800, 380);
    let shot = Shot::load(dir.path(), &name).unwrap();
    assert_eq!((shot.picture.width, shot.picture.height), (800, 380));
    assert_eq!(shot.picture.pixels.len(), 800 * 380 * 4);
    assert_eq!(&shot.picture.pixels[..4], &[17, 17, 25, 255]);
    assert!(
        shot.problems(&name).is_empty(),
        "{:?}",
        shot.problems(&name)
    );
}

#[test]
fn a_shot_whose_files_disagree_is_inconsistent() {
    let dir = tempfile::tempdir().unwrap();
    let name = Name::parse("mac.welcome.b").unwrap();
    let list = LIST.replacen("\"parent\": 1", "\"parent\": 2", 1);
    write_pair(dir.path(), &name, &list, 640, 380);
    let problems = Shot::load(dir.path(), &name).unwrap().problems(&name);
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(problems[0].contains("names the board mac.welcome.a"));
    assert!(problems[1].contains("640x380 pixels, not the list's 800x380"));
    assert!(problems[2].contains("element 2 has a parent"));
}

#[test]
fn a_picture_within_a_pixel_of_the_list_size_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let name = Name::parse("mac.welcome.a").unwrap();
    write_pair(dir.path(), &name, LIST, 799, 381);
    assert!(
        Shot::load(dir.path(), &name)
            .unwrap()
            .problems(&name)
            .is_empty()
    );
}

#[test]
fn unreadable_files_fail_to_load() {
    let dir = tempfile::tempdir().unwrap();
    let name = Name::parse("mac.welcome.a").unwrap();
    assert!(matches!(
        Shot::load(dir.path(), &name),
        Err(ShotError::Read(..))
    ));
    write_pair(dir.path(), &name, "{}", 800, 380);
    assert!(matches!(
        Shot::load(dir.path(), &name),
        Err(ShotError::List(..))
    ));
    write_pair(dir.path(), &name, LIST, 800, 380);
    std::fs::write(dir.path().join(name.file_name("png")), "not a png").unwrap();
    assert!(matches!(
        Shot::load(dir.path(), &name),
        Err(ShotError::Picture(..))
    ));
}

#[test]
fn words_are_composed_collapsed_and_trimmed_and_nothing_else() {
    assert_eq!(words("  Cafe\u{301}  \n au   lait "), "Caf\u{e9} au lait");
    assert_eq!(
        words("\u{201c}Set up\u{201d}\u{2026}"),
        "\u{201c}Set up\u{201d}\u{2026}"
    );
}

fn window(radius: f64, exponent: f64) -> WindowShape {
    WindowShape {
        frame: Bounds {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 60.0,
        },
        radius,
        exponent,
    }
}

#[test]
fn a_window_covers_its_frame_and_leaves_out_what_its_corners_cut() {
    let round = window(20.0, 2.0);
    assert!(round.contains(60.0, 50.0));
    assert!(!round.contains(9.0, 50.0), "left of the frame");
    assert!(!round.contains(60.0, 81.0), "below the frame");
    assert!(!round.contains(10.5, 20.5), "the tip of a round corner");
    // A circle of radius 20 centred 20 in from each edge reaches 20 * (1 - 1/sqrt 2).
    assert!(round.contains(10.0 + 6.0, 20.0 + 6.0));
    assert!(!round.contains(10.0 + 5.0, 20.0 + 5.0));
    assert!(
        !round.contains(110.0 - 5.0, 80.0 - 5.0),
        "every corner is cut"
    );
}

#[test]
fn a_straight_cut_corner_drops_the_triangle_at_the_tip() {
    let cut = window(20.0, 1.0);
    assert!(cut.contains(10.0 + 11.0, 20.0 + 10.0));
    assert!(!cut.contains(10.0 + 9.0, 20.0 + 10.0));
}

#[test]
fn a_window_with_no_radius_covers_its_whole_frame() {
    assert!(window(0.0, 2.0).contains(10.0, 20.0));
}
