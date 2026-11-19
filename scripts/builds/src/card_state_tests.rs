use super::*;
use crate::lists::Id;
use crate::lists::fixtures::{NOON, db, item};

fn seen(worktree: &str, state: &'static str, items: &[u32]) -> Seen {
    Seen {
        worktree: worktree.to_string(),
        state,
        items: items.iter().copied().collect(),
    }
}

fn rows(db: &Connection) -> Vec<(String, i64, String)> {
    db.prepare("SELECT worktree, at, state FROM card_state ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

const SIDE: &str = "/d/bae-worktrees/side-3";

#[test]
fn a_card_gets_a_row_only_when_its_state_or_items_change() {
    let mut db = db();
    let id = item(&db, vec![]).0;
    let none = BTreeSet::new();
    follow(&mut db, NOON + 10, &[seen(SIDE, "working", &[id])], &none).unwrap();
    follow(&mut db, NOON + 20, &[seen(SIDE, "working", &[id])], &none).unwrap();
    follow(
        &mut db,
        NOON + 30,
        &[seen(SIDE, "needs-review", &[id])],
        &none,
    )
    .unwrap();
    follow(
        &mut db,
        NOON + 40,
        &[seen(SIDE, "needs-review", &[])],
        &none,
    )
    .unwrap();
    assert_eq!(
        rows(&db),
        vec![
            (SIDE.to_string(), NOON + 10, "working".to_string()),
            (SIDE.to_string(), NOON + 30, "needs-review".to_string()),
            (SIDE.to_string(), NOON + 40, "needs-review".to_string()),
        ]
    );
    assert_eq!(
        read_item(&db, Id(id), NOON + 99).unwrap(),
        vec![
            Stretch {
                worktree: SIDE.to_string(),
                state: "working".to_string(),
                start: NOON + 10,
                end: NOON + 30,
            },
            Stretch {
                worktree: SIDE.to_string(),
                state: "needs-review".to_string(),
                start: NOON + 30,
                end: NOON + 40,
            },
        ],
        "the card stopped listing the item at NOON + 40"
    );
}

#[test]
fn a_card_that_leaves_the_project_is_gone_unless_the_daemon_could_not_choose_its_project() {
    let mut db = db();
    let other = "/d/bae-worktrees/side-4";
    let none = BTreeSet::new();
    follow(
        &mut db,
        NOON + 10,
        &[seen(SIDE, "working", &[]), seen(other, "free", &[])],
        &none,
    )
    .unwrap();
    let unread: BTreeSet<String> = [other.to_string()].into();
    follow(&mut db, NOON + 20, &[], &unread).unwrap();
    follow(&mut db, NOON + 30, &[], &unread).unwrap();
    assert_eq!(
        rows(&db)[2..],
        [(SIDE.to_string(), NOON + 20, GONE.to_string())],
        "one gone row, and none for the card whose project is unread"
    );
}

#[test]
fn a_write_deletes_the_old_rows_but_keeps_each_worktrees_last() {
    let mut db = db();
    let other = "/d/bae-worktrees/side-4";
    let none = BTreeSet::new();
    follow(
        &mut db,
        NOON,
        &[seen(SIDE, "working", &[]), seen(other, "free", &[])],
        &none,
    )
    .unwrap();
    follow(
        &mut db,
        NOON + 10,
        &[seen(SIDE, "paused", &[]), seen(other, "free", &[])],
        &none,
    )
    .unwrap();
    let later = NOON + KEEP_DAYS * 86400 + 5;
    follow(
        &mut db,
        later,
        &[seen(SIDE, "paused", &[]), seen(other, "free", &[])],
        &none,
    )
    .unwrap();
    assert_eq!(
        rows(&db),
        vec![
            (other.to_string(), NOON, "free".to_string()),
            (SIDE.to_string(), NOON + 10, "paused".to_string()),
        ]
    );
}
