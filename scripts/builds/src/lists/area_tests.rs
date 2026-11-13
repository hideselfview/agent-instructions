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
    let before = demo.state();
    assert!(refusal(area(&demo, &["add", "Alpha"])).contains("exists already"));
    assert!(refusal(area(&demo, &["add", "X", "--after", "Nope"])).contains("does not exist"));
    assert!(refusal(area(&demo, &["add", " "])).contains("empty"));
    assert_eq!(demo.state(), before);
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
    let before = demo.state();
    let refused = refusal(area(&demo, &["remove", "Alpha"]));
    assert!(refused.contains("still holds #1"), "{refused}");
    assert_eq!(demo.state(), before);
    assert_eq!(names(&demo), ["Alpha", "Beta"]);
}

#[test]
fn rename_follows_the_closed_items_too() {
    let demo = demo_with_a_closed_item("area-rename-closed");
    area(&demo, &["rename", "Alpha", "First"]).unwrap();
    assert_eq!(closed_area(&demo, 1), "First");
    assert!(demo.store.load().is_ok());
}

#[test]
fn every_named_constraint_of_the_area_table_refuses_its_row() {
    use crate::lists::fixtures::{Refusals, db, row, v};
    let db = db();
    let area = [("name", v("Gamma")), ("position", v(7))];
    plant(&db, &area).unwrap();
    let mut refused = Refusals::new(include_str!("area.sql"));
    for name in ["", " x", "x ", "a\nb", "a\tb"] {
        refused.check("area_name_is_one_line", plant(&db, &row(&area, &[("name", v(name))])));
    }
    refused.check("area_position_not_negative", plant(&db, &row(&area, &[("name", v("Delta")), ("position", v(-1))])));
    refused.check_as("area_position_is_its_own", "UNIQUE constraint failed: area.position", plant(&db, &row(&area, &[("name", v("Delta"))])));
    refused.done();
}

#[test]
fn a_rename_carries_every_item_open_or_closed_through_the_foreign_key() {
    use crate::lists::fixtures::{db, item, read, stamp};
    let db = db();
    let open = item(&db, vec![]);
    let closed = item(&db, vec![]);
    crate::lists::record::close(&db, closed, crate::lists::How::Dropped("x".into()), &stamp(1)).unwrap();
    rename_area(&db, "Alpha", "Renamed").unwrap();
    let lists = read(&db);
    assert_eq!(lists.areas, ["Renamed", "Beta"]);
    assert_eq!(lists.item(open).unwrap().area, "Renamed");
    let Some(crate::lists::Closed { record: crate::lists::Record::Item(item), .. }) = lists.closed_record(closed) else { panic!() };
    assert_eq!(item.area, "Renamed");
}

#[test]
fn moving_areas_renumbers_without_the_unique_position_tripping() {
    use crate::lists::fixtures::db;
    let db = db();
    add_area(&db, "Gamma", None).unwrap();
    move_area(&db, "Gamma", "Alpha").unwrap();
    assert_eq!(areas(&db).unwrap(), ["Alpha", "Gamma", "Beta"]);
    add_area(&db, "Delta", Some("Alpha")).unwrap();
    assert_eq!(areas(&db).unwrap(), ["Alpha", "Delta", "Gamma", "Beta"]);
    move_area(&db, "Alpha", "Beta").unwrap();
    assert_eq!(areas(&db).unwrap(), ["Delta", "Gamma", "Beta", "Alpha"]);
}
