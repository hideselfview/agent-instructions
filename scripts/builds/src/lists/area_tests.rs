use super::*;
use crate::lists::item::cmd_item;
use crate::lists::item_fixtures::{Demo, demo, refusal, words};

fn area(demo: &Demo, args: &[&str]) -> Result<()> {
    cmd_area(&demo.tool, &words(args))
}

fn names(demo: &Demo) -> Vec<String> {
    demo.store.load().unwrap().areas
}

#[test]
fn add_places_an_area_at_the_end_or_after_another() {
    let demo = demo("area-add");
    area(&demo, &["add", "Gamma"]).unwrap();
    area(&demo, &["add", "Between", "--after", "Alpha"]).unwrap();
    assert_eq!(names(&demo), ["Alpha", "Between", "Beta", "Gamma"]);
    let before = demo.bytes();
    assert!(refusal(area(&demo, &["add", "Alpha"])).contains("exists already"));
    assert!(refusal(area(&demo, &["add", "X", "--after", "Nope"])).contains("does not exist"));
    assert!(refusal(area(&demo, &["add", " "])).contains("empty"));
    assert_eq!(demo.bytes(), before);
}

#[test]
fn rename_follows_the_open_items_and_move_reorders() {
    let demo = demo("area-rename");
    let details = demo.brief("d");
    cmd_item(&demo.tool, &words(&["add", "--title", "Do the first thing now", "--area", "Alpha", "--details-file", &details])).unwrap();
    area(&demo, &["rename", "Alpha", "First"]).unwrap();
    assert_eq!(demo.store.load().unwrap().items[0].area, "First");
    assert!(refusal(area(&demo, &["rename", "First", "Beta"])).contains("exists already"));
    area(&demo, &["move", "First", "--after", "Beta"]).unwrap();
    assert_eq!(names(&demo), ["Beta", "First"]);
    assert!(refusal(area(&demo, &["move", "First", "--after", "First"])).contains("after itself"));
    assert!(refusal(area(&demo, &["move", "First"])).contains("--after is required"));
}

#[test]
fn remove_is_refused_while_an_item_is_in_the_area() {
    let demo = demo("area-remove");
    let details = demo.brief("d");
    cmd_item(&demo.tool, &words(&["add", "--title", "Do the first thing now", "--area", "Alpha", "--details-file", &details])).unwrap();
    let refused = refusal(area(&demo, &["remove", "Alpha"]));
    assert!(refused.contains("still holds #1") && refused.contains("item edit N --area"), "{refused}");
    area(&demo, &["remove", "Beta"]).unwrap();
    assert_eq!(names(&demo), ["Alpha"]);
}
