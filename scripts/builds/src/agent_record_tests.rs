use super::*;

fn agent(session: &str) -> Agent {
    Agent {
        session: session.into(),
        work: Work::Implementing {
            places: vec![Place {
                worktree: "/w/side-3".into(),
                branch: "work".into(),
            }],
        },
        model: "claude-sonnet-5-5".into(),
        tier: Tier::Implementation,
        folder: "/w/side-3".into(),
        brief: "plans/briefs/a.md".into(),
        started_at: 1000,
        started_by: "coordinator".into(),
        items: vec![1, 2],
        context: None,
    }
}

fn process(pid: i32) -> Process {
    Process {
        pid,
        pid_started: Some(999),
        started_at: 1000,
        message: "h".into(),
    }
}

/// A store with items 1 and 2, which the records of agents point at.
fn db_with_items() -> Connection {
    let db = crate::lists::fixtures::db();
    crate::lists::fixtures::item(&db, vec![]);
    crate::lists::fixtures::item(&db, vec![]);
    db
}

#[test]
fn starting_an_agent_records_it_its_places_its_items_and_its_first_process() {
    let db = db_with_items();
    start(&db, &agent("s1"), &process(50)).unwrap();
    let row: (String, String, i64) = db
        .query_row(
            "SELECT role, model, started_at FROM agent WHERE session = 's1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        ("implementing".into(), "claude-sonnet-5-5".into(), 1000)
    );
    let read = read(&db, "s1").unwrap().unwrap();
    assert_eq!(read, agent("s1"));
    let items: Vec<i64> = db
        .prepare("SELECT item FROM agent_item WHERE session = 's1' ORDER BY item")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(items, [1, 2]);
    let number: i64 = db
        .query_row(
            "SELECT number FROM agent_process WHERE session = 's1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(number, 1);
}

#[test]
fn an_agent_for_an_item_that_does_not_exist_is_refused_whole() {
    let db = db_with_items();
    let mut bad = agent("s1");
    bad.items = vec![1, 99];
    assert!(start(&db, &bad, &process(50)).is_err());
}

#[test]
fn a_process_ends_once_and_the_context_keeps_the_last_turns_tokens_and_the_window() {
    let db = db_with_items();
    start(&db, &agent("s1"), &process(50)).unwrap();
    set_tokens(&db, "s1", 100).unwrap();
    set_tokens(&db, "s1", 292_000).unwrap();
    set_window(&db, "s1", 1_000_000).unwrap();
    set_tokens(&db, "s1", 293_000).unwrap();
    let context = read(&db, "s1").unwrap().unwrap().context;
    assert_eq!(
        context,
        Some(Context {
            tokens: 293_000,
            window: Some(1_000_000),
        })
    );
    end_process(&db, "s1", 1, 1010, 0).unwrap();
    assert!(end_process(&db, "s1", 1, 1011, 1).is_err());
}
