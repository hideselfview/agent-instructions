use super::*;
use crate::testing::{Scratch, git as run_git, repo};

fn head(dir: &Path) -> String {
    git_text(dir, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    run_git(dir, &["add", file]);
    run_git(dir, &["commit", "-q", "-m", file]);
    head(dir)
}

#[test]
fn a_cherry_picked_commit_keeps_its_patch_id_and_an_empty_commit_has_none() {
    let scratch = Scratch::new("review-commits");
    let dir = scratch.dir.join("repo");
    repo(&dir, "target 1G\n");
    let base = head(&dir);
    std::fs::write(dir.join("other.txt"), "other\n").unwrap();
    run_git(&dir, &["checkout", "-q", "-b", "work"]);
    let a = commit(&dir, "a.txt", "a\n");
    run_git(&dir, &["commit", "-q", "--allow-empty", "-m", "empty"]);
    let empty = head(&dir);
    let b = commit(&dir, "b.txt", "b\n");
    let work = with_patch_ids(&dir, &in_range(&dir, &base, "work").unwrap()).unwrap();
    assert_eq!(
        work.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(),
        [a.as_str(), b.as_str()],
        "the empty commit {empty} has no patch id"
    );
    // The same two changes picked onto a main that moved: new SHAs, the same
    // patch ids.
    run_git(&dir, &["checkout", "-q", "main"]);
    commit(&dir, "c.txt", "c\n");
    run_git(&dir, &["cherry-pick", &a, &b]);
    let picked = with_patch_ids(&dir, &in_range(&dir, "HEAD~2", "HEAD").unwrap()).unwrap();
    assert_ne!(picked[0].sha, a);
    assert!(same_change(&work, &picked));
    assert!(!same_change(&work, &picked[..1]));
    let reversed: Vec<Commit> = work.iter().rev().cloned().collect();
    assert!(!same_change(&work, &reversed), "the order counts");
    assert_eq!(
        paths(&dir, &[a.clone(), empty, b.clone()]).unwrap(),
        ["a.txt", "b.txt"]
    );
    assert!(with_patch_ids(&dir, &["no-such-commit".to_string()]).is_err());
}

#[test]
fn a_commit_rebased_over_a_changed_context_line_keeps_its_patch_id_and_a_changed_added_line_does_not()
 {
    let scratch = Scratch::new("review-commits-context");
    let dir = scratch.dir.join("repo");
    repo(&dir, "target 1G\n");
    let lines = |changed: &[(usize, &str)]| -> String {
        (1..=9)
            .map(|n| {
                changed
                    .iter()
                    .find(|(at, _)| *at == n)
                    .map_or(format!("line {n}\n"), |(_, text)| format!("{text}\n"))
            })
            .collect()
    };
    commit(&dir, "f.txt", &lines(&[]));
    run_git(&dir, &["checkout", "-q", "-b", "work"]);
    let reviewed = commit(&dir, "f.txt", &lines(&[(5, "line five")]));
    // Main changes line 3, which a diff shows as context around the change to
    // line 5. The test then cherry-picks the change onto main.
    run_git(&dir, &["checkout", "-q", "main"]);
    commit(&dir, "f.txt", &lines(&[(3, "line three")]));
    run_git(&dir, &["cherry-pick", &reviewed]);
    let rebased = head(&dir);
    let read = with_patch_ids(&dir, &[reviewed, rebased]).unwrap();
    assert_eq!(
        read[0].patch_id, read[1].patch_id,
        "only a context line changed"
    );
    // A conflict's resolution that changes the added line is new work.
    std::fs::write(
        dir.join("f.txt"),
        lines(&[(3, "line three"), (5, "line FIVE")]),
    )
    .unwrap();
    run_git(&dir, &["commit", "-q", "--amend", "-a", "--no-edit"]);
    let resolved = with_patch_ids(&dir, &[head(&dir)]).unwrap();
    assert_ne!(
        read[0].patch_id, resolved[0].patch_id,
        "the added line changed"
    );
}
