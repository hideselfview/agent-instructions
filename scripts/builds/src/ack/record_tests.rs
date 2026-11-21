use super::*;

/// Each part's made-up words, at the places 0, 1, 2 and on of their part.
fn words(parts: &[&[&str]]) -> Vec<Vec<MadeUp>> {
    parts
        .iter()
        .map(|part| {
            part.iter()
                .enumerate()
                .map(|(place, word)| MadeUp {
                    place,
                    word: word.to_string(),
                })
                .collect()
        })
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
    acknowledge(&db, "s1", "agent", None, "v1", 5, &[]).unwrap();
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
    acknowledge(&db, "s1", "agent", None, "v2", 12, &sections()).unwrap();
    assert_eq!(
        last(&db, "s1", "agent", None).unwrap(),
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
    acknowledge(&db, "s1", "agent", None, "v0", 0, &sections()).unwrap();
    for n in 1..KEPT_VERSIONS {
        acknowledge(&db, "s1", "other", None, &format!("v{n}"), n, &sections()).unwrap();
    }
    acknowledge(&db, "s2", "again", None, "v0", KEPT_VERSIONS, &sections()).unwrap();
    acknowledge(
        &db,
        "s1",
        "other",
        None,
        "new",
        KEPT_VERSIONS + 1,
        &sections(),
    )
    .unwrap();
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
fn forgetting_an_agent_deletes_its_acknowledgement_and_reading_only() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "a1", None, "v1", 5, &[]).unwrap();
    acknowledge(&db, "s1", "a2", None, "v1", 6, &[]).unwrap();
    start_reading(
        &db,
        "s1",
        "a1",
        "v2",
        8,
        &names(&["builds help"]),
        &words(&[&["ka"]]),
    )
    .unwrap();
    forget(&db, "s1", "a1").unwrap();
    assert_eq!(last(&db, "s1", "a1", None).unwrap(), None);
    assert_eq!(reading(&db, "s1", "a1").unwrap(), None);
    assert!(last(&db, "s1", "a2", None).unwrap().is_some());
}

#[test]
fn a_made_up_word_holds_only_lowercase_letters() {
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

#[test]
fn an_agent_keeps_one_acknowledgement_of_each_repos_text_and_a_compaction_clears_them_all() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "a1", Some("bae"), "bae1", 5, &[]).unwrap();
    acknowledge(&db, "s1", "a1", Some("coven"), "coven1", 6, &[]).unwrap();
    acknowledge(&db, "s1", "a1", None, "none1", 7, &[]).unwrap();
    acknowledge(&db, "s1", "a1", Some("bae"), "bae2", 8, &[]).unwrap();
    acknowledge(&db, "s1", "a1", None, "none2", 9, &[]).unwrap();
    let of = |repo: Option<&str>| last(&db, "s1", "a1", repo).unwrap().unwrap();
    assert_eq!(of(Some("bae")).version, "bae2");
    assert_eq!(of(Some("coven")).version, "coven1");
    assert_eq!(of(None).version, "none2");
    let rows: i64 = db
        .query_row("SELECT COUNT(*) FROM ack", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 3);
    forget(&db, "s1", "a1").unwrap();
    for repo in [Some("bae"), Some("coven"), None] {
        assert_eq!(last(&db, "s1", "a1", repo).unwrap(), None);
    }
}
