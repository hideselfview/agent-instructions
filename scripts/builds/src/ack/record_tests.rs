use super::*;

fn words(parts: &[&[&str]]) -> Vec<Vec<String>> {
    parts
        .iter()
        .map(|part| part.iter().map(|word| word.to_string()).collect())
        .collect()
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn sections() -> Vec<(String, String)> {
    vec![
        ("builds help".to_string(), "aa".to_string()),
        ("builds help lists".to_string(), "bb".to_string()),
    ]
}

#[test]
fn a_reading_keeps_its_sections_and_each_parts_words_until_the_next_reading_replaces_them() {
    let db = crate::db::memory();
    start_reading(
        &db,
        "s1",
        "agent",
        "v1",
        10,
        &names(&["builds help lists", "builds help"]),
        &words(&[&["ka", "mo"], &["ti"]]),
    )
    .unwrap();
    assert_eq!(
        reading(&db, "s1", "agent").unwrap(),
        Some(Reading {
            version: "v1".to_string(),
            sections: names(&["builds help lists", "builds help"]),
            words: words(&[&["ka", "mo"], &["ti"]]),
        })
    );
    start_reading(
        &db,
        "s1",
        "agent",
        "v2",
        11,
        &names(&["builds help"]),
        &words(&[&["ru"]]),
    )
    .unwrap();
    let replaced = reading(&db, "s1", "agent").unwrap().unwrap();
    assert_eq!(replaced.sections, names(&["builds help"]));
    assert_eq!(replaced.words, words(&[&["ru"]]));
    assert_eq!(reading(&db, "s1", "other").unwrap(), None);
}

#[test]
fn an_acknowledgement_replaces_the_last_one_keeps_its_version_and_ends_the_reading() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "agent", "v1", 5, &[]).unwrap();
    start_reading(
        &db,
        "s1",
        "agent",
        "v2",
        10,
        &names(&["builds help"]),
        &words(&[&["ka"]]),
    )
    .unwrap();
    acknowledge(&db, "s1", "agent", "v2", 12, &sections()).unwrap();
    assert_eq!(
        last(&db, "s1", "agent").unwrap(),
        Some(Ack {
            version: "v2".to_string(),
            at: 12,
        })
    );
    assert_eq!(kept_sections(&db, "v2").unwrap(), Some(sections()));
    assert_eq!(kept_sections(&db, "v1").unwrap(), Some(Vec::new()));
    assert_eq!(kept_sections(&db, "v3").unwrap(), None);
    assert_eq!(reading(&db, "s1", "agent").unwrap(), None);
}

#[test]
fn builds_keeps_only_the_versions_agents_acknowledged_last() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "agent", "v0", 0, &sections()).unwrap();
    for n in 1..KEPT_VERSIONS {
        acknowledge(&db, "s1", "other", &format!("v{n}"), n, &sections()).unwrap();
    }
    acknowledge(&db, "s2", "again", "v0", KEPT_VERSIONS, &sections()).unwrap();
    acknowledge(&db, "s1", "other", "new", KEPT_VERSIONS + 1, &sections()).unwrap();
    assert_eq!(kept_sections(&db, "v1").unwrap(), None);
    assert!(kept_sections(&db, "v0").unwrap().is_some());
    assert!(kept_sections(&db, "new").unwrap().is_some());
    let gone: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM ack_version_section WHERE version = 'v1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(gone, 0);
}

#[test]
fn the_last_who_is_the_newest_acknowledgement_or_reading_of_the_session() {
    let db = crate::db::memory();
    assert_eq!(last_who(&db, "s1").unwrap(), None);
    acknowledge(&db, "s1", "coordinator", "v1", 5, &[]).unwrap();
    start_reading(
        &db,
        "s1",
        "agent",
        "v1",
        8,
        &names(&["builds help"]),
        &words(&[&["ka"]]),
    )
    .unwrap();
    acknowledge(&db, "s2", "elsewhere", "v1", 9, &[]).unwrap();
    assert_eq!(last_who(&db, "s1").unwrap().as_deref(), Some("agent"));
}

#[test]
fn a_check_word_holds_only_lowercase_letters() {
    let db = crate::db::memory();
    assert!(
        start_reading(
            &db,
            "s1",
            "agent",
            "v1",
            8,
            &names(&["builds help"]),
            &words(&[&["Ka1"]])
        )
        .is_err()
    );
}
