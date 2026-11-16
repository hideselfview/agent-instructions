use super::*;
use crate::fail::Fail;
use crate::lists::check::faults;
use crate::lists::fixtures::{NOON, db, design, item, question, read, stamp, v};
use crate::lists::item::{ItemDraft, add_item, set_origin};
use crate::lists::tag::add_tags;
use crate::lists::{Audience, Kind, question};

fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result.unwrap_err() {
        Fail::Message(message) => message,
        Fail::Exit(code) => panic!("exit {code}"),
    }
}

fn tags(list: &[&str]) -> Vec<String> {
    list.iter().map(|tag| tag.to_string()).collect()
}

/// The tags record <id> carries itself.
fn own_tags(db: &Connection, id: Id) -> Vec<String> {
    crate::lists::tag::read(db)
        .unwrap()
        .remove(&id)
        .unwrap_or_default()
}

#[test]
fn a_trail_runs_from_the_first_record_and_shows_its_tags_on_every_later_one() {
    let db = db();
    let first = question(&db);
    add_tags(&db, first, &tags(&["screen-reads audit"])).unwrap();
    let work = item(&db, vec![]);
    set_origin(&db, work, first).unwrap();
    let asked = question(&db);
    question::set_origin(&db, asked, work).unwrap();
    let lists = read(&db);
    assert_eq!(
        trail(&lists, asked),
        Trail {
            tags: tags(&["screen-reads audit"]),
            earlier: vec![first, work],
        }
    );
    assert_eq!(
        trail_line(&lists, asked).unwrap(),
        format!("trail: screen-reads audit → {first} → {work} Do the work numbered 2 → {asked}")
    );
    assert!(trail(&lists, work).wanted(Some("screen-reads audit")));
    assert!(!trail(&lists, work).wanted(Some("other audit")));
    assert_eq!(
        trail_line(&lists, first).unwrap(),
        format!("trail: screen-reads audit → {first}")
    );
    // A record with no origin and no tag has no trail to show.
    let alone = item(&db, vec![]);
    assert_eq!(trail_line(&read(&db), alone), None);
}

#[test]
fn an_item_a_design_change_made_comes_from_it() {
    let db = db();
    let change = design(&db);
    let draft = ItemDraft {
        title: "Build the board as drawn".into(),
        area: "Alpha".into(),
        details: "details".into(),
        r#for: Audience::Coordinator,
        waits: Vec::new(),
        brief: None,
        priority: None,
        tags: Vec::new(),
    };
    let made = add_item(&db, &draft, Some(change), &stamp(NOON)).unwrap();
    let lists = read(&db);
    assert_eq!(lists.item(made).unwrap().origin, Some(change));
    assert_eq!(
        trail_line(&lists, made).unwrap(),
        format!("trail: {change} Design change {} → {made}", change.0)
    );
}

#[test]
fn an_origin_must_be_another_record_whose_trail_does_not_come_back() {
    let db = db();
    let first = question(&db);
    let work = item(&db, vec![]);
    set_origin(&db, work, first).unwrap();
    let later = item(&db, vec![]);
    set_origin(&db, later, work).unwrap();
    assert!(refusal(set_origin(&db, work, work)).contains("cannot come from itself"));
    assert!(refusal(set_origin(&db, work, Id(99))).contains("is not a record"));
    let message = refusal(question::set_origin(&db, first, later));
    assert!(
        message.contains(&format!("{first} cannot come from {later}")),
        "{message}"
    );
    assert!(refusal(set_origin(&db, first, later)).contains("is not an item"));
    assert!(refusal(question::set_origin(&db, work, first)).contains("is not a question"));
}

#[test]
fn a_tag_the_trail_already_shows_is_refused_and_a_tag_on_an_earlier_record_leaves_the_later_ones() {
    let db = db();
    let first = question(&db);
    add_tags(&db, first, &tags(&["a audit"])).unwrap();
    let work = item(&db, vec![]);
    set_origin(&db, work, first).unwrap();
    let message = refusal(add_tags(&db, work, &tags(&["a audit"])));
    assert!(
        message.contains(&format!(
            "{work}'s trail already shows the tag 'a audit': {first} carries it"
        )),
        "{message}"
    );
    add_tags(&db, work, &tags(&["b audit"])).unwrap();
    add_tags(&db, first, &tags(&["b audit"])).unwrap();
    assert_eq!(own_tags(&db, work), Vec::<String>::new());
    assert_eq!(own_tags(&db, first), tags(&["a audit", "b audit"]));
    assert_eq!(trail(&read(&db), work).tags, tags(&["a audit", "b audit"]));
}

#[test]
fn a_new_origin_takes_off_the_tags_its_trail_shows_from_the_record_and_the_ones_after_it() {
    let db = db();
    let first = question(&db);
    add_tags(&db, first, &tags(&["a audit", "c audit"])).unwrap();
    let work = item(&db, vec![]);
    add_tags(&db, work, &tags(&["a audit", "b audit"])).unwrap();
    let later = item(&db, vec![]);
    set_origin(&db, later, work).unwrap();
    add_tags(&db, later, &tags(&["c audit"])).unwrap();
    set_origin(&db, work, first).unwrap();
    assert_eq!(own_tags(&db, work), tags(&["b audit"]));
    assert_eq!(own_tags(&db, later), Vec::<String>::new());
    assert_eq!(
        trail(&read(&db), later).tags,
        tags(&["a audit", "c audit", "b audit"])
    );
    assert_eq!(faults(&db, "demo").unwrap(), Vec::<String>::new());
}

#[test]
fn the_check_names_a_loop_of_origins_and_a_tag_the_trail_already_shows() {
    let db = db();
    for id in [10_i64, 11] {
        crate::lists::record::plant(
            &db,
            "record",
            &[
                ("id", v(id)),
                ("kind", v("question")),
                ("opened_at", v(NOON)),
                ("opened_by", v("coordinator")),
            ],
        )
        .unwrap();
    }
    for (id, origin) in [(10_i64, 11_i64), (11, 10)] {
        crate::lists::question::plant(
            &db,
            "question",
            &[
                ("id", v(id)),
                ("title", v("A loop")),
                ("details", v("d")),
                ("kind", v("question")),
                ("blocks_nothing", v("nothing waits")),
                ("origin", v(origin)),
            ],
        )
        .unwrap();
    }
    let first = question(&db);
    add_tags(&db, first, &tags(&["a audit"])).unwrap();
    let work = item(&db, vec![]);
    set_origin(&db, work, first).unwrap();
    crate::lists::tag::plant(
        &db,
        "record_tag",
        &[
            ("record_id", v(work.0)),
            ("kind", v(Kind::Item.word())),
            ("tag", v("a audit")),
        ],
    )
    .unwrap();
    assert_eq!(
        faults(&db, "demo").unwrap(),
        [
            "record #10 comes from itself through its chain of origins".to_string(),
            "record #11 comes from itself through its chain of origins".to_string(),
            format!(
                "record {work} carries the tag a audit, which {first} earlier in its trail carries"
            ),
        ]
    );
    // The loop's trail stops at the first record seen twice.
    assert_eq!(trail(&read(&db), Id(10)).earlier, vec![Id(11)]);
}
