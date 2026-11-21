use super::*;

fn tokens(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| token.to_string()).collect()
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
fn a_reading_keeps_its_sections_and_each_parts_tokens_until_the_next_reading_replaces_them() {
    let db = crate::db::memory();
    start_reading(
        &db,
        "s1",
        "agent",
        "v1",
        10,
        &names(&["builds help lists", "builds help"]),
        &tokens(&["t0", "t1", "t2"]),
    )
    .unwrap();
    assert_eq!(
        reading(&db, "s1", "agent").unwrap(),
        Some(Reading {
            version: "v1".to_string(),
            sections: names(&["builds help lists", "builds help"]),
            tokens: tokens(&["t0", "t1", "t2"]),
        })
    );
    start_reading(
        &db,
        "s1",
        "agent",
        "v2",
        11,
        &names(&["builds help"]),
        &tokens(&["u0", "u1"]),
    )
    .unwrap();
    let replaced = reading(&db, "s1", "agent").unwrap().unwrap();
    assert_eq!(replaced.sections, names(&["builds help"]));
    assert_eq!(replaced.tokens, tokens(&["u0", "u1"]));
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
        &tokens(&["k0", "k1"]),
    )
    .unwrap();
    acknowledge(&db, "s1", "agent", "v2", 12, &sections()).unwrap();
    assert_eq!(last(&db, "s1", "agent").unwrap(), Some("v2".to_string()));
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
fn forgetting_an_agent_deletes_its_acknowledgement_and_reading_only() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "a1", "v1", 5, &[]).unwrap();
    acknowledge(&db, "s1", "a2", "v1", 6, &[]).unwrap();
    start_reading(
        &db,
        "s1",
        "a1",
        "v2",
        8,
        &names(&["builds help"]),
        &tokens(&["k0", "k1"]),
    )
    .unwrap();
    forget(&db, "s1", "a1").unwrap();
    assert_eq!(last(&db, "s1", "a1").unwrap(), None);
    assert_eq!(reading(&db, "s1", "a1").unwrap(), None);
    assert!(last(&db, "s1", "a2").unwrap().is_some());
}

#[test]
fn a_token_is_not_empty() {
    let db = crate::db::memory();
    assert!(
        start_reading(
            &db,
            "s1",
            "agent",
            "v1",
            8,
            &names(&["builds help"]),
            &tokens(&[""])
        )
        .is_err()
    );
}

/// Writes two acknowledgements, one keyed by repo and one with no repo, into
/// the schema from before `ack` left the repo: the state migrate.rs's move of
/// that table starts from. The current schema has no `repo` column, so no
/// write of this file's can build them.
pub fn plant_acks_keyed_by_repo(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "INSERT INTO ack (session, agent, repo, version, at) VALUES ('s', 'a', 'bae', 'v1', 5), ('s', 'a', NULL, 'v2', 6)",
    )
}
