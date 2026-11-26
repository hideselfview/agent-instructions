use super::*;

fn agent(session: &str) -> Agent {
    Agent {
        session: session.into(),
        role: AgentRole::Implementing,
        model: "claude-sonnet-5-5".into(),
        tier: Tier::Implementation,
        folder: "/w/side-3".into(),
        brief: Some("plans/briefs/a.md".into()),
        places: vec![("/w/side-3".into(), "work".into())],
        started_at: 1000,
        started_by: "coordinator".into(),
        items: vec![1, 2],
        context: None,
    }
}

fn process(pid: i32) -> Process {
    Process {
        pid,
        pid_started: 999,
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
fn starting_an_agent_records_it_its_items_and_its_first_process() {
    let db = db_with_items();
    start(&db, &agent("s1"), &process(50)).unwrap();
    let row: (String, String, String, String, i64) = db
        .query_row(
            "SELECT role, model, worktree, branch, started_at FROM agent WHERE session = 's1'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "implementing".into(),
            "claude-sonnet-5-5".into(),
            "/w/side-3".into(),
            "work".into(),
            1000
        )
    );
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
    set_context(&db, "s1", 100, None, 1001).unwrap();
    set_context(&db, "s1", 292_000, Some(1_000_000), 1002).unwrap();
    set_context(&db, "s1", 293_000, None, 1003).unwrap();
    let context: (i64, i64, i64) = db
        .query_row(
            "SELECT context_tokens, context_window, context_read_at FROM agent WHERE session = 's1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(context, (293_000, 1_000_000, 1003));
    end_process(&db, "s1", 1, 1010, 0).unwrap();
    assert!(end_process(&db, "s1", 1, 1011, 1).is_err());
}
