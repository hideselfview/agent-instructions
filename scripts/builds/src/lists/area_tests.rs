use super::*;
use crate::lists::item_command::cmd_item;
use crate::lists::item_fixtures::{Demo, demo, refusal, words};
use crate::lists::{Id, Record};

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

/// A demo with item 1 added in Alpha and dropped, so it is closed.
fn demo_with_a_closed_item(name: &str) -> Demo {
    let demo = demo(name);
    let details = demo.brief("d");
    cmd_item(&demo.tool, &words(&["add", "--title", "Do the first thing now", "--area", "Alpha", "--details-file", &details])).unwrap();
    cmd_item(&demo.tool, &words(&["drop", "1", "--why", "not needed"])).unwrap();
    demo
}

fn closed_area(demo: &Demo, id: u32) -> String {
    let lists = demo.store.load().unwrap();
    let Record::Item(item) = &lists.closed_record(Id(id)).unwrap().record else { panic!("an item") };
    item.area.clone()
}

#[test]
fn remove_is_refused_while_a_closed_item_is_in_the_area() {
    let demo = demo_with_a_closed_item("area-remove-closed");
    let before = demo.bytes();
    let refused = refusal(area(&demo, &["remove", "Alpha"]));
    assert!(refused.contains("still holds #1"), "{refused}");
    assert_eq!(demo.bytes(), before);
    assert_eq!(names(&demo), ["Alpha", "Beta"]);
}

#[test]
fn rename_follows_the_closed_items_too() {
    let demo = demo_with_a_closed_item("area-rename-closed");
    area(&demo, &["rename", "Alpha", "First"]).unwrap();
    assert_eq!(closed_area(&demo, 1), "First");
    assert!(demo.store.load().is_ok());
}
