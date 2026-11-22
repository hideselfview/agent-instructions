use super::*;

fn entry(worktree: &str) -> Entry {
    Entry {
        worktree: worktree.to_string(),
        shas: vec!["abc".to_string()],
        rev: "a".to_string(),
        start: "branch a".to_string(),
        lanes: vec!["mac".to_string()],
    }
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

#[test]
fn the_queue_lists_waiting_landings_oldest_first_and_skips_settled_ones() {
    let scratch = crate::testing::Scratch::new("land-batch-queue");
    let dir = queue_dir(&scratch.dir, Lane::Mac);
    let one = register(&dir, &entry("one")).unwrap();
    let two = register(&dir, &entry("two")).unwrap();
    let three = register(&dir, &entry("three")).unwrap();
    let ids: Vec<String> = waiting(&dir)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, [one.id.clone(), two.id.clone(), three.id.clone()]);
    let verdict = Verdict::Landed {
        on_main: vec!["def".to_string()],
        said: "def a".to_string(),
    };
    write_verdict(&dir, &two.id, &verdict).unwrap();
    let ids: Vec<String> = waiting(&dir)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, [one.id.clone(), three.id.clone()]);
    assert_eq!(two.verdict().unwrap(), Some(verdict));
    assert_eq!(one.verdict().unwrap(), None);
}

#[test]
fn an_entry_that_no_process_locks_belongs_to_an_ended_landing_and_is_removed() {
    let scratch = crate::testing::Scratch::new("land-batch-dead");
    let dir = queue_dir(&scratch.dir, Lane::Mac);
    let live = register(&dir, &entry("live")).unwrap();
    // A landing that ended without removing its files left no lock behind.
    let dead_path = dir.join("0-dead.entry");
    std::fs::write(&dead_path, serde_json::to_string(&entry("dead")).unwrap()).unwrap();
    std::fs::write(dir.join("0-dead.verdict"), "{}").unwrap();
    std::fs::write(dir.join("orphan.verdict"), "{}").unwrap();
    let found = waiting(&dir).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, live.id);
    assert!(!dead_path.exists());
    assert!(!dir.join("orphan.verdict").exists());
    assert!(!dir.join("0-dead.verdict").exists());
}

#[test]
fn a_missing_queue_has_no_waiting_landing() {
    let scratch = crate::testing::Scratch::new("land-batch-missing");
    assert!(
        waiting(&queue_dir(&scratch.dir, Lane::Android))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn merged_builds_run_every_step_once_and_every_pick_once() {
    let merged = merge_builds(&[
        words("bae r1 macos --test mac CardTests --shots mac card."),
        words(
            "bae r1 macos macos-shots --test mac CardTests --test mac RowTests --all-shots android",
        ),
        words("bae r1 macos --all-shots android"),
    ]);
    assert_eq!(
        merged,
        words(
            "bae r1 macos macos-shots --test mac CardTests --shots mac card. --test mac RowTests --all-shots android"
        )
    );
}

#[test]
fn one_build_merges_to_itself() {
    let one = words("bae r1 macos --test mac CardTests");
    assert_eq!(merge_builds(std::slice::from_ref(&one)), one);
}
