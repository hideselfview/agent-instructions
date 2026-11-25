use super::*;

fn tokens(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| token.to_string()).collect()
}

fn sections() -> Vec<(String, String)> {
    vec![
        ("builds help".to_string(), "aa".to_string()),
        ("builds help lists".to_string(), "bb".to_string()),
    ]
}

#[test]
fn a_reading_keeps_each_parts_tokens_until_the_next_reading_replaces_them() {
    let db = crate::db::memory();
    start_reading(&db, "s1", "agent", "v1", 10, &tokens(&["t0", "t1", "t2"])).unwrap();
    assert_eq!(
        reading(&db, "s1", "agent").unwrap(),
        Some(Reading {
            version: "v1".to_string(),
            tokens: tokens(&["t0", "t1", "t2"]),
        })
    );
    start_reading(&db, "s1", "agent", "v2", 11, &tokens(&["u0", "u1"])).unwrap();
    let replaced = reading(&db, "s1", "agent").unwrap().unwrap();
    assert_eq!(replaced.tokens, tokens(&["u0", "u1"]));
    assert_eq!(reading(&db, "s1", "other").unwrap(), None);
}

#[test]
fn an_acknowledgement_replaces_the_last_one_keeps_its_version_and_ends_the_reading() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "agent", "v1", 5, &[]).unwrap();
    start_reading(&db, "s1", "agent", "v2", 10, &tokens(&["k0", "k1"])).unwrap();
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
    start_reading(&db, "s1", "a1", "v2", 8, &tokens(&["k0", "k1"])).unwrap();
    forget(&db, "s1", "a1").unwrap();
    assert_eq!(last(&db, "s1", "a1").unwrap(), None);
    assert_eq!(reading(&db, "s1", "a1").unwrap(), None);
    assert!(last(&db, "s1", "a2").unwrap().is_some());
}

#[test]
fn the_acknowledged_sections_are_those_of_the_agents_last_kept_version() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "agent", "v1", 5, &sections()).unwrap();
    assert_eq!(
        acknowledged_sections(&db, "s1", "agent").unwrap(),
        sections()
    );
    assert!(
        acknowledged_sections(&db, "s1", "other")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn builds_keeps_a_version_that_an_agents_acknowledgement_names() {
    let db = crate::db::memory();
    acknowledge(&db, "s1", "agent", "old", 0, &sections()).unwrap();
    for n in 1..=KEPT_VERSIONS * 2 {
        acknowledge(&db, "s1", "other", &format!("v{n}"), n, &sections()).unwrap();
    }
    assert!(kept_sections(&db, "old").unwrap().is_some());
    assert_eq!(kept_sections(&db, "v1").unwrap(), None);
}

#[test]
fn a_token_is_not_empty() {
    let db = crate::db::memory();
    assert!(start_reading(&db, "s1", "agent", "v1", 8, &tokens(&[""])).is_err());
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

/// Writes an acknowledgement, a reading with a section and a made-up word into
/// the schema from before the readings were chained by tokens: the state
/// migrate.rs's move of that table starts from.
pub fn plant_before_tokens(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "INSERT INTO ack (session, agent, version, at) VALUES ('s', 'a', 'v1', 5);
         INSERT INTO ack_reading (session, agent, version, started_at) VALUES ('s', 'a', 'v1', 6);
         INSERT INTO ack_reading_section (session, agent, position, name) VALUES ('s', 'a', 1, 'help');
         INSERT INTO ack_word (session, agent, part, position, word) VALUES ('s', 'a', 1, 1, 'kodamir');",
    )
}

/// Writes an acknowledgement with its time, and a reading with a section and
/// a token, into the schema from before nothing read the time or the
/// sections: the state migrate.rs's move of those tables starts from.
pub fn plant_before_unread(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "INSERT INTO ack (session, agent, version, at) VALUES ('s', 'a', 'v1', 5);
         INSERT INTO ack_reading (session, agent, version, started_at) VALUES ('s', 'a', 'v2', 6);
         INSERT INTO ack_reading_section (session, agent, position, name) VALUES ('s', 'a', 1, 'help');
         INSERT INTO ack_token (session, agent, part, token) VALUES ('s', 'a', 0, 't0');",
    )
}

/// Deletes every kept version, as a store from before builds kept every
/// acknowledged version has none for an old acknowledgement.
pub fn forget_every_version(db: &Connection) {
    db.execute("DELETE FROM ack_version", []).unwrap();
}
