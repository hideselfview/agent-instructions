use super::*;
use crate::testing::{Scratch, age};

/// Texts of <project> in a folder of <scratch>.
pub fn texts_in(scratch: &Scratch, project: &str) -> Texts {
    Texts::of(&scratch.places(), project)
}

#[test]
fn a_texts_hash_names_its_content_and_length() {
    assert_eq!(hash("abc"), hash("abc"));
    assert_ne!(hash("abc"), hash("abd"));
    assert!(hash("abc").ends_with("-3"));
}

#[test]
fn a_run_keeps_each_text_once_under_its_hash_and_a_read_returns_it() {
    let scratch = Scratch::new("texts-keep");
    let texts = texts_in(&scratch, "demo");
    texts.keep_run(7, ["system", "message", "system"]).unwrap();
    let folder = scratch.places().root.join("review-texts/demo/7");
    // Two different texts and the mark of use.
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 3);
    assert_eq!(texts.read(7, &hash("message")).unwrap(), "message");
    // The same run kept again replaces its texts whole.
    texts.keep_run(7, ["other"]).unwrap();
    assert!(texts.read(7, &hash("message")).is_err());
    assert_eq!(texts.read(7, &hash("other")).unwrap(), "other");
}

#[test]
fn a_text_the_cap_deleted_is_a_failure_that_says_so() {
    let scratch = Scratch::new("texts-gone");
    let texts = texts_in(&scratch, "demo");
    texts.keep_run(1, ["a"]).unwrap();
    let why = texts.read(2, &hash("a")).unwrap_err().text();
    assert!(
        why.contains("keeps no text") && why.contains("review run 2"),
        "{why}"
    );
    assert!(texts.read(1, &hash("b")).is_err());
}

#[test]
fn projects_keep_their_runs_apart() {
    let scratch = Scratch::new("texts-projects");
    texts_in(&scratch, "bae").keep_run(1, ["x"]).unwrap();
    assert!(texts_in(&scratch, "coven").read(1, &hash("x")).is_err());
}

#[test]
fn over_the_cap_the_runs_read_longest_ago_lose_their_texts_first() {
    let scratch = Scratch::new("texts-cap");
    let tool = scratch.tool(0);
    let texts = texts_in(&scratch, "demo");
    let root = root(&tool.places);
    let big = "x".repeat(400 * 1024);
    for (run, hours) in [(1, 30), (2, 10), (3, 20), (4, 1)] {
        texts.keep_run(run, [big.as_str()]).unwrap();
        age(&root.join(format!("demo/{run}/{USED}")), hours);
    }
    // A read marks run 1 used, so it outlasts the older runs.
    texts.read(1, &hash(&big)).unwrap();
    prune_texts(&tool, Run::Real, 1000).unwrap();
    let kept: Vec<bool> = (1..=4)
        .map(|run| root.join(format!("demo/{run}")).exists())
        .collect();
    assert_eq!(kept, [true, false, false, true]);
}

#[test]
fn a_run_kept_in_the_last_half_hour_stays_over_the_cap() {
    let scratch = Scratch::new("texts-recent");
    let tool = scratch.tool(0);
    let texts = texts_in(&scratch, "demo");
    texts
        .keep_run(1, ["x".repeat(400 * 1024).as_str()])
        .unwrap();
    prune_texts(&tool, Run::Real, 100).unwrap();
    assert!(texts.read(1, &hash(&"x".repeat(400 * 1024))).is_ok());
}

#[test]
fn a_folder_with_no_texts_yet_prunes_to_nothing() {
    let scratch = Scratch::new("texts-none");
    let tool = scratch.tool(0);
    prune_texts(&tool, Run::Real, 0).unwrap();
}
