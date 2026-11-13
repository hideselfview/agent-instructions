use super::*;
use crate::checkout::Checkout;
use crate::lists::Running;
use crate::lists::fixtures::{NOON, item, lists, stamp};
use crate::testing::Scratch;
use std::path::PathBuf;

fn root_at(top: &Path, claimed: bool) -> Root {
    let git_dir = top.join(".git-of-side");
    std::fs::create_dir_all(&git_dir).unwrap();
    let checkout = Checkout { top: top.to_path_buf(), main: top.to_path_buf(), git_dir };
    if claimed {
        Claim { who: "agent".into(), since: "2026-11-13 10:00".into(), note: None, note_state: None }
            .write(&checkout.claim_file())
            .unwrap();
    }
    Root { path: PathBuf::from("/volume/bae/side-1"), top: top.to_path_buf(), live: Some(checkout) }
}

fn running_in(worktree: &Path) -> Vec<Loaded> {
    let mut state = lists();
    let id = item(&mut state, vec![]);
    state.item_mut(id).unwrap().running = Some(Running {
        worktree: worktree.display().to_string(),
        branch: "work".into(),
        brief: "b.md".into(),
        since: stamp(NOON),
    });
    vec![Loaded { project: "demo".into(), lists: Ok(state) }]
}

#[test]
fn a_running_item_in_a_claimed_worktree_is_fine() {
    let scratch = Scratch::new("running-ok");
    let top = scratch.dir.join("bae-worktrees/side-1");
    let found = findings(&Ok(running_in(&top)), &[root_at(&top, true)]).unwrap();
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_running_item_in_an_unclaimed_or_gone_worktree_is_named() {
    let scratch = Scratch::new("running-bad");
    let top = scratch.dir.join("bae-worktrees/side-1");
    let unclaimed = findings(&Ok(running_in(&top)), &[root_at(&top, false)]).unwrap();
    assert_eq!(unclaimed.len(), 1);
    assert!(unclaimed[0].contains("has no claim") && unclaimed[0].contains("`builds item stop 1`"), "{unclaimed:?}");
    let gone = findings(&Ok(running_in(&top)), &[]).unwrap();
    assert!(gone[0].contains("is not a live worktree"), "{gone:?}");
}

#[test]
fn a_store_failing_its_check_is_a_finding() {
    let bad = vec![Loaded { project: "demo".into(), lists: Err("ids repeat".into()) }];
    assert_eq!(findings(&Ok(bad), &[]).unwrap(), vec!["lists: project demo fails its check: ids repeat".to_string()]);
    assert_eq!(findings(&Err("listing failed".into()), &[]).unwrap(), vec!["lists: listing failed".to_string()]);
}
