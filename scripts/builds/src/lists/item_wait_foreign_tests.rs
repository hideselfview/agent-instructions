use super::*;
use crate::lists::derive::{ItemState, item_state, unmet_waits, wait_met};
use crate::lists::fixtures::{NOON, make_store_named, stamp};
use crate::lists::item_command::cmd_item;
use crate::lists::item_fixtures::{Demo, demo, refusal, words};
use crate::lists::record::close;
use crate::lists::store::{Store, reload};

const FIVE: &str = "Do the first thing now";

fn item(demo: &Demo, args: &[&str]) -> Result<()> {
    cmd_item(&demo.tool, &crate::testing::no_ssh(), &words(args))
}

/// Project `other`, holding open item 1, beside the demo project, whose item
/// 1 is open too.
fn two_projects(name: &str) -> (Demo, Store) {
    let demo = demo(name);
    let other = make_store_named(&demo.tool.places, "other", &["elsewhere"]);
    other
        .write(&stamp(NOON), "item add", |db| {
            Ok(crate::lists::fixtures::item(db, Vec::new()))
        })
        .unwrap();
    let details = demo.brief("details.txt");
    item(
        &demo,
        &[
            "add",
            "--title",
            FIVE,
            "--area",
            "Alpha",
            "--details-file",
            &details,
        ],
    )
    .unwrap();
    (demo, other)
}

fn close_in(store: &Store, id: u32, how: How) {
    store
        .write(&stamp(NOON + 60), "close", |db| {
            close(db, Id(id), how, &stamp(NOON + 60))
        })
        .unwrap();
}

#[test]
fn a_wait_on_an_item_of_another_project_clears_when_that_item_lands() {
    let (demo, other) = two_projects("foreign-wait-lands");
    item(&demo, &["wait", "1", "--on", "item:other/1"]).unwrap();
    let lists = demo.store.load().unwrap();
    let wait = Wait::Foreign {
        project: "other".into(),
        item: Id(1),
    };
    assert_eq!(
        lists.item(Id(1)).unwrap().waits,
        std::slice::from_ref(&wait)
    );
    assert_eq!(unmet_waits(&lists, lists.item(Id(1)).unwrap()), [&wait]);
    assert_eq!(
        item_state(&lists, lists.item(Id(1)).unwrap()),
        ItemState::Waiting
    );
    assert_eq!(
        lists.foreign_item("other", Id(1)).unwrap().standing(),
        Some(&ForeignStanding::Open)
    );

    close_in(&other, 1, How::Landed(vec!["a".repeat(40)]));
    let lists = demo.store.load().unwrap();
    assert!(wait_met(&lists, &wait));
    assert!(unmet_waits(&lists, lists.item(Id(1)).unwrap()).is_empty());
    assert_eq!(
        item_state(&lists, lists.item(Id(1)).unwrap()),
        ItemState::Ready
    );
    assert_eq!(
        lists.foreign_item("other", Id(1)).unwrap().standing(),
        Some(&ForeignStanding::Met(NOON + 60))
    );
}

#[test]
fn a_wait_on_an_item_that_closed_without_landing_stays_unmet_and_says_so() {
    let (demo, other) = two_projects("foreign-wait-dropped");
    item(&demo, &["wait", "1", "--on", "item:other/1"]).unwrap();
    close_in(&other, 1, How::Dropped("not needed".into()));
    let lists = demo.store.load().unwrap();
    let wait = lists.item(Id(1)).unwrap().waits[0].clone();
    assert!(!wait_met(&lists, &wait));
    assert!(
        crate::lists::show::wait_words(&lists, &wait).contains("closed as dropped"),
        "{}",
        crate::lists::show::wait_words(&lists, &wait)
    );
    item(&demo, &["wait", "1", "--off", "item:other/1"]).unwrap();
    assert!(
        demo.store
            .load()
            .unwrap()
            .item(Id(1))
            .unwrap()
            .waits
            .is_empty()
    );
}

#[test]
fn a_wait_names_an_open_item_of_another_project_or_is_refused() {
    let (demo, other) = two_projects("foreign-wait-refused");
    assert!(refusal(item(&demo, &["wait", "1", "--on", "item:demo/1"])).contains("own project"));
    assert!(refusal(item(&demo, &["wait", "1", "--on", "item:nowhere/1"])).contains("nowhere"));
    assert!(refusal(item(&demo, &["wait", "1", "--on", "item:other/9"])).contains("no item"));
    assert!(refusal(item(&demo, &["wait", "1", "--on", "item:../1"])).contains("not a project"));
    close_in(&other, 1, How::Landed(vec!["b".repeat(40)]));
    assert!(refusal(item(&demo, &["wait", "1", "--on", "item:other/1"])).contains("closed"));
    assert!(
        demo.store
            .load()
            .unwrap()
            .item(Id(1))
            .unwrap()
            .waits
            .is_empty()
    );
}

#[test]
fn the_daemon_reloads_a_store_when_the_item_it_waits_on_lands() {
    let (demo, other) = two_projects("foreign-wait-reload");
    item(&demo, &["wait", "1", "--on", "item:other/1"]).unwrap();
    let databases = crate::lists::store::database_paths(&demo.tool.places).unwrap();
    let first = Ok(crate::lists::store::load_stores(
        &crate::lists::store::stores(&demo.tool.places).unwrap(),
    ));
    close_in(&other, 1, How::Landed(vec!["c".repeat(40)]));
    let (second, changed) = reload(&demo.tool.places, &databases, &first);
    assert!(changed);
    let demo_lists = second
        .unwrap()
        .into_iter()
        .find(|loaded| loaded.project == "demo")
        .unwrap()
        .lists
        .unwrap();
    assert!(demo_lists.foreign_item("other", Id(1)).unwrap().met());
}

/// Health follows a wait on an item of another project to that item, as it
/// follows a wait on an item of the same project, so the waiting item needs
/// no hand-ended condition and gets no row of its own.
#[test]
fn health_lists_the_item_of_the_other_project_not_the_item_that_waits_on_it() {
    let (mut demo, _other) = two_projects("foreign-wait-health");
    item(&demo, &["wait", "1", "--on", "item:other/1"]).unwrap();
    demo.tool.now = NOON + 2 * 60 * 60;
    let mut health = crate::health::health(&demo.tool).unwrap();
    let loaded = crate::lists::store::load_all(&demo.tool.places);
    crate::health_stillness::append(
        &mut health,
        &loaded,
        &crate::health_stillness::Progress::read(&demo.tool.places, &loaded),
        None,
        &demo.tool.places,
        demo.tool.now,
        demo.tool.rules.still_minutes,
    );
    let lines = crate::health::lines(&health);
    let rows: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("item #"))
        .collect();
    assert_eq!(rows.len(), 1, "{lines:?}");
    assert!(rows[0].contains("It blocks 1 item."), "{lines:?}");
}

/// The lists a write reads inside its transaction hold the other project's
/// item too, so a write judges the wait as a load does.
#[test]
fn the_lists_a_write_reads_see_the_item_of_the_other_project_landed() {
    let (demo, other) = two_projects("foreign-wait-in-write");
    item(&demo, &["wait", "1", "--on", "item:other/1"]).unwrap();
    close_in(&other, 1, How::Landed(vec!["d".repeat(40)]));
    let met = demo
        .store
        .write(&stamp(NOON + 120), "read", |db| {
            let lists = crate::lists::store::read(db)?;
            Ok(unmet_waits(&lists, lists.item(Id(1)).unwrap()).is_empty())
        })
        .unwrap();
    assert!(met);
}
