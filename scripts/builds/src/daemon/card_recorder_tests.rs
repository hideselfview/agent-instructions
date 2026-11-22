use super::*;
use crate::daemon::cards::{CardState, ReviewWait};

fn card(worktree: &str, lists: CardLists, state: CardState, items: &[u32]) -> CardNow {
    CardNow {
        worktree: worktree.to_string(),
        lists,
        state,
        items: items.to_vec(),
    }
}

#[test]
fn each_card_goes_to_its_projects_store_by_its_states_name_and_an_unread_one_to_none() {
    let (shown, unread) = by_project(&[
        card(
            "/d/bae-worktrees/side-1",
            CardLists::Project("bae".into()),
            CardState::InReview(ReviewWait::ToSettle(
                3,
                crate::daemon::cards::Settler::Judge,
            )),
            &[12],
        ),
        card(
            "/d/agent-instructions-worktrees/side-2",
            CardLists::Unread,
            CardState::Free,
            &[],
        ),
        card(
            "/d/other-worktrees/side-1",
            CardLists::None,
            CardState::Working,
            &[],
        ),
        card(
            "/d/bae-worktrees/side-2",
            CardLists::Project("bae".into()),
            CardState::InReview(ReviewWait::DesignCoverage),
            &[13],
        ),
    ]);
    assert_eq!(
        shown,
        BTreeMap::from([(
            "bae".to_string(),
            vec![
                Seen {
                    worktree: "/d/bae-worktrees/side-1".into(),
                    state: "findings-to-settle",
                    items: BTreeSet::from([12]),
                },
                Seen {
                    worktree: "/d/bae-worktrees/side-2".into(),
                    state: "reviewing",
                    items: BTreeSet::from([13]),
                },
            ]
        )])
    );
    assert_eq!(
        unread,
        BTreeSet::from(["/d/agent-instructions-worktrees/side-2".to_string()])
    );
}
