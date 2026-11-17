use super::*;

fn words(parts: &[&[&str]]) -> Vec<Vec<String>> {
    parts
        .iter()
        .map(|part| part.iter().map(|word| word.to_string()).collect())
        .collect()
}

fn sections() -> Vec<(String, String)> {
    vec![
        ("builds help".to_string(), "aa".to_string()),
        ("builds help lists".to_string(), "bb".to_string()),
    ]
}

#[test]
fn a_reading_keeps_each_parts_words_in_order_until_the_next_reading_replaces_them() {
    let db = crate::db::memory();
    start_reading(
        &db,
        "s1",
        "agent",
        "v1",
        10,
        &words(&[&["ka", "mo"], &["ti"]]),
    )
    .unwrap();
    assert_eq!(
        reading(&db, "s1", "agent").unwrap(),
        Some(Reading {
            version: "v1".to_string(),
            words: words(&[&["ka", "mo"], &["ti"]]),
        })
    );
    start_reading(&db, "s1", "agent", "v2", 11, &words(&[&["ru"]])).unwrap();
    assert_eq!(
        reading(&db, "s1", "agent").unwrap().unwrap().words,
        words(&[&["ru"]])
    );
    assert_eq!(reading(&db, "s1", "other").unwrap(), None);
}

#[test]
fn an_acknowledgement_replaces_the_last_one_and_ends_the_reading() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "agent", "v1", 5, &[]).unwrap();
    start_reading(&db, "s1", "agent", "v2", 10, &words(&[&["ka"]])).unwrap();
    acknowledge(&db, "s1", "agent", "v2", 12, &sections()).unwrap();
    assert_eq!(
        last(&db, "s1", "agent").unwrap(),
        Some(Ack {
            session: "s1".to_string(),
            who: "agent".to_string(),
            version: "v2".to_string(),
            at: 12,
            sections: sections(),
        })
    );
    assert_eq!(reading(&db, "s1", "agent").unwrap(), None);
}

#[test]
fn the_last_who_is_the_newest_acknowledgement_or_reading_of_the_session() {
    let db = crate::db::memory();
    assert_eq!(last_who(&db, "s1").unwrap(), None);
    acknowledge(&db, "s1", "coordinator", "v1", 5, &[]).unwrap();
    start_reading(&db, "s1", "agent", "v1", 8, &words(&[&["ka"]])).unwrap();
    acknowledge(&db, "s2", "elsewhere", "v1", 9, &[]).unwrap();
    assert_eq!(last_who(&db, "s1").unwrap().as_deref(), Some("agent"));
}

#[test]
fn forgetting_a_session_deletes_its_acknowledgements_and_readings_only() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "coordinator", "v1", 5, &sections()).unwrap();
    acknowledge(&db, "s1", "agent", "v1", 6, &[]).unwrap();
    start_reading(&db, "s1", "agent", "v1", 8, &words(&[&["ka"]])).unwrap();
    acknowledge(&db, "s2", "elsewhere", "v1", 9, &[]).unwrap();
    assert_eq!(forget_session(&db, "s1").unwrap(), 2);
    assert_eq!(last(&db, "s1", "coordinator").unwrap(), None);
    assert_eq!(reading(&db, "s1", "agent").unwrap(), None);
    assert!(last(&db, "s2", "elsewhere").unwrap().is_some());
}

#[test]
fn a_check_word_holds_only_lowercase_letters() {
    let db = crate::db::memory();
    assert!(start_reading(&db, "s1", "agent", "v1", 8, &words(&[&["Ka1"]])).is_err());
}
