use super::*;
use crate::claim::Claim;
use crate::daemon::model::Activity;
use crate::side_git::{SideGit, SideRead};

fn card(
    state: Option<&'static str>,
    claim: Option<Claim>,
    uncommitted: Option<usize>,
) -> CheckoutPage {
    CheckoutPage {
        name: "side-3".into(),
        path: "/d/bae-worktrees/side-3".into(),
        root: "/v/bae/side-3".into(),
        live: true,
        main: false,
        branch: Some("branch".into()),
        app_checkout: false,
        app_lane: None,
        claim,
        git: uncommitted.map(|uncommitted| {
            SideGit::Side(Ok(SideRead {
                head: None,
                ahead: Vec::new(),
                main: "0".repeat(40),
                uncommitted,
                last_write: None,
            }))
        }),
        state: state.map(str::to_string),
        folders: None,
        total_kib: None,
        activity: Activity::default(),
        words: Words::default(),
        landings: None,
        errors: Vec::new(),
    }
}

/// The card state that the page group <word> names in these tests: in-review
/// stands for Needs review, and queued for Waiting on Mac.
fn st(word: &str) -> CardState {
    match word {
        "working" => CardState::Working,
        "ready-to-land" => CardState::ReadyToLand,
        "held" => CardState::Paused,
        "winding-down" => CardState::WindingDown,
        "waiting-on-you" => CardState::WaitingOnYou,
        "free" => CardState::Free,
        "queued" => CardState::Queued(Lane::Mac),
        "waits-unread" => CardState::WaitsUnread,
        "in-review" => CardState::InReview(ReviewWait::NotRun),
        other => panic!("no card state for {other}"),
    }
}

fn claim(note: Option<&str>) -> Option<Claim> {
    Some(Claim {
        who: "agent".into(),
        since: 1_790_000_000,
        note: note.map(|text| crate::claim::Note::Plain(text.to_string())),
    })
}

#[test]
fn the_chip_names_only_a_worktree_with_an_agent_or_commits() {
    let chip = |state| {
        words(
            &card(Some(state), claim(None), Some(0)),
            st(state),
            &[],
            &[],
        )
        .chip
    };
    assert_eq!(chip("ready-to-land"), Some("Ready to land".to_string()));
    assert_eq!(chip("working"), Some("Working".to_string()));
    assert_eq!(chip("held"), Some("Paused".to_string()));
    assert_eq!(chip("winding-down"), Some("Working".to_string()));
    assert_eq!(chip("waiting-on-you"), Some("Waiting on you".to_string()));
    assert_eq!(chip("free"), None);
    let mut main = card(None, None, None);
    main.main = true;
    assert_eq!(main_words(&main).chip, None);
}

#[test]
fn with_no_running_item_the_card_shows_the_agents_note_or_what_the_state_means() {
    let note = |state, note| {
        words(
            &card(Some(state), claim(note), Some(0)),
            st(state),
            &[],
            &[],
        )
        .note
    };
    assert_eq!(
        note("working", Some("Measuring each screen read.")).as_deref(),
        Some("Measuring each screen read.")
    );
    assert_eq!(
        note("working", None).as_deref(),
        Some("Working; its agent left no note.")
    );
    assert_eq!(
        note("ready-to-land", None).as_deref(),
        Some("Waits for its agent to land it.")
    );
    assert_eq!(
        note("ready-to-land", Some("Done.")).as_deref(),
        Some("Done.")
    );
    assert_eq!(
        note("free", None).as_deref(),
        Some("No agent claims this worktree and every commit has landed.")
    );
    assert_eq!(
        words(
            &card(Some("ready-to-land"), None, Some(0)),
            st("ready-to-land"),
            &[],
            &[]
        )
        .note
        .as_deref(),
        Some("Commits wait to land and no agent claims this worktree.")
    );
    let mut main = card(None, None, None);
    main.main = true;
    assert_eq!(
        main_words(&main).note.as_deref(),
        Some("Your checkout. No agent works or builds here.")
    );
}

#[test]
fn the_footer_counts_uncommitted_files_and_says_nothing_for_a_clean_tree() {
    let files = |n| {
        words(
            &card(Some("working"), claim(None), n),
            st("working"),
            &[],
            &[],
        )
        .uncommitted
    };
    let count = |text: &str, title: &str| {
        Some(Uncommitted {
            text: text.to_string(),
            title: title.to_string(),
        })
    };
    assert_eq!(files(Some(0)), None);
    assert_eq!(
        files(Some(1)),
        count("1 file", "1 file changed, not committed")
    );
    assert_eq!(
        files(Some(7)),
        count("7 files", "7 files changed, not committed")
    );
    assert_eq!(files(None), None);
    let mut broken = card(Some("working"), claim(None), Some(3));
    broken.git = Some(SideGit::Side(Err("git failed".into())));
    assert_eq!(
        words(&broken, CardState::Working, &[], &[]).uncommitted,
        None
    );
}

fn running(items: &[(u32, &str)]) -> Vec<RunningItem> {
    items
        .iter()
        .map(|(id, title)| RunningItem {
            id: *id,
            title: title.to_string(),
        })
        .collect()
}

fn listed(state: &'static str, note: Option<&str>, items: &[(u32, &str)]) -> Words {
    words(
        &card(Some(state), claim(note), Some(0)),
        st(state),
        &running(items),
        &[],
    )
}

#[test]
fn the_card_lists_each_running_item_by_number_and_title_and_shows_no_note() {
    for state in ["working", "waiting-on-you", "winding-down", "ready-to-land"] {
        let card = listed(
            state,
            Some("Measuring each screen read."),
            &[(117, "Test the storage manager")],
        );
        assert_eq!(
            card.items,
            running(&[(117, "Test the storage manager")]),
            "{state}"
        );
        assert_eq!(card.note, None, "{state} shows no note beside its items");
    }
    let batch = listed(
        "working",
        Some("note"),
        &[(159, "First item"), (160, "Second item")],
    );
    assert_eq!(
        batch.items,
        running(&[(159, "First item"), (160, "Second item")])
    );
    assert_eq!(batch.note, None);
}

#[test]
fn a_card_with_no_running_item_shows_its_note_and_no_items() {
    let card = listed("working", Some("Measuring each screen read."), &[]);
    assert!(card.items.is_empty());
    assert_eq!(card.note.as_deref(), Some("Measuring each screen read."));
}

#[test]
fn a_card_that_waits_on_nothing_has_no_pill() {
    for state in ["working", "ready-to-land", "winding-down", "free"] {
        assert_eq!(
            listed(state, Some("note"), &[(1, "Item")]).pill,
            None,
            "{state}"
        );
    }
}

fn waiting(state: &'static str, wait: ItemWait) -> Words {
    words(
        &card(Some(state), claim(Some("note")), Some(0)),
        st(state),
        &running(&[(7, "Reopen a copied library")]),
        &[wait],
    )
}

fn pill(text: &str, title: &str) -> Option<Pill> {
    Some(Pill {
        text: text.to_string(),
        title: title.to_string(),
    })
}

#[test]
fn an_item_that_waits_on_a_question_names_it_in_the_pill_under_waiting_on_you() {
    let card = waiting(
        "waiting-on-you",
        ItemWait::Answer {
            id: 438,
            title: "When to reopen a copied library".into(),
        },
    );
    assert_eq!(card.chip, Some("Waiting on you".to_string()));
    assert_eq!(
        card.pill,
        pill(
            "Answer #438",
            "Waits on your answer to #438: When to reopen a copied library"
        )
    );
    assert_eq!(card.items, running(&[(7, "Reopen a copied library")]));
    assert_eq!(card.note, None);
}

#[test]
fn an_item_that_waits_on_another_item_names_it_in_the_pill_and_stays_working() {
    let card = waiting(
        "working",
        ItemWait::Item {
            id: 1320,
            title: "Store the library id".into(),
            state: "ready",
        },
    );
    assert_eq!(card.chip, Some("Working".to_string()));
    assert_eq!(
        card.pill,
        pill(
            "After #1320",
            "Waits on #1320 (ready): Store the library id"
        )
    );
}

#[test]
fn an_item_that_waits_on_an_approved_design_change_names_it_in_the_pill_and_stays_working() {
    let card = waiting(
        "working",
        ItemWait::Landing {
            id: 428,
            title: "Heading row".into(),
        },
    );
    assert_eq!(card.chip, Some("Working".to_string()));
    assert_eq!(
        card.pill,
        pill(
            "After #428",
            "Waits on design change 428 to land: Heading row"
        )
    );
}

#[test]
fn an_item_that_waits_on_a_design_review_names_the_change_in_the_pill() {
    let card = waiting(
        "waiting-on-you",
        ItemWait::Review {
            id: 428,
            title: "Heading row".into(),
        },
    );
    assert_eq!(
        card.pill,
        pill(
            "Review #428",
            "Waits on your review of design change 428: Heading row"
        )
    );
}

#[test]
fn a_design_branch_no_recorded_change_covers_says_it_waits_on_a_design_change() {
    let card = words(
        &card(Some("ready-to-land"), claim(Some("note")), Some(0)),
        st("ready-to-land"),
        &[],
        &[ItemWait::Unrecorded],
    );
    assert_eq!(card.chip, Some("Ready to land".to_string()));
    assert_eq!(
        card.pill,
        pill(
            "No design change",
            "Waits until a recorded design change covers the design files its commits change."
        )
    );
}

#[test]
fn a_design_branch_with_no_running_item_shows_its_note_and_the_change_in_the_pill() {
    let review = ItemWait::Review {
        id: 1419,
        title: "AirPlay PIN sheet".into(),
    };
    let noted = words(
        &card(
            Some("waiting-on-you"),
            claim(Some("Drawing the sheet.")),
            Some(0),
        ),
        st("waiting-on-you"),
        &[],
        std::slice::from_ref(&review),
    );
    assert!(noted.items.is_empty());
    assert_eq!(noted.note.as_deref(), Some("Drawing the sheet."));
    assert_eq!(
        noted.pill,
        pill(
            "Review #1419",
            "Waits on your review of design change 1419: AirPlay PIN sheet"
        )
    );
    let unnoted = words(
        &card(Some("waiting-on-you"), claim(None), Some(0)),
        st("waiting-on-you"),
        &[],
        std::slice::from_ref(&review),
    );
    assert_eq!(unnoted.note, None, "the pill holds the sentence");
}

#[test]
fn a_card_whose_pill_holds_the_sentence_shows_no_note_without_a_claim_note() {
    let answer = ItemWait::Answer {
        id: 438,
        title: "Which port?".into(),
    };
    let asked = words(
        &card(Some("waiting-on-you"), claim(None), Some(0)),
        st("waiting-on-you"),
        &[],
        &[answer],
    );
    assert!(asked.pill.is_some());
    assert_eq!(asked.note, None);
    let unrecorded = words(
        &card(Some("ready-to-land"), claim(None), Some(0)),
        st("ready-to-land"),
        &[],
        &[ItemWait::Unrecorded],
    );
    assert_eq!(unrecorded.note, None);
}

#[test]
fn a_held_worktree_says_paused_and_what_its_running_items_wait_on() {
    let waits = [
        ItemWait::Item {
            id: 1320,
            title: "Store the library id".into(),
            state: "ready",
        },
        ItemWait::Condition {
            text: "builds #122, #121 and #125 land".into(),
            until: crate::lists::Ender::Coordinator,
        },
    ];
    let card = words(
        &card(Some("held"), claim(Some("Until the store lands.")), Some(0)),
        st("held"),
        &running(&[(3, "Item")]),
        &waits,
    );
    assert_eq!(card.chip, Some("Paused".to_string()));
    assert_eq!(card.pill, None);
    assert_eq!(card.items, running(&[(3, "Item")]));
    assert_eq!(
        card.note.as_deref(),
        Some(
            "Waits on #1320 (ready): Store the library id. Waits until builds #122, #121 and #125 land; the coordinator ends this wait"
        )
    );
    let noted = listed("held", Some("Until the store lands."), &[(3, "Item")]);
    assert_eq!(noted.chip, Some("Paused".to_string()));
    assert_eq!(noted.note.as_deref(), Some("Until the store lands."));
    let unnoted = listed("held", None, &[]);
    assert_eq!(
        unnoted.note.as_deref(),
        Some("Its agent holds this worktree on purpose and runs no work.")
    );
}

#[test]
fn a_condition_ranks_after_every_other_wait_and_has_no_pill() {
    let condition = ItemWait::Condition {
        text: "builds #122 lands".into(),
        until: crate::lists::Ender::Coordinator,
    };
    for other in [
        ItemWait::Item {
            id: 1,
            title: "Other".into(),
            state: "ready",
        },
        ItemWait::Unrecorded,
        ItemWait::RulesReview(ReviewWait::NotRun),
    ] {
        assert!(other.rank() < condition.rank(), "{other:?}");
    }
    assert_eq!(waiting("working", condition).pill, None);
}

#[test]
fn ranked_waits_come_once_each_in_rank_order() {
    let condition = ItemWait::Condition {
        text: "builds #122 lands".into(),
        until: crate::lists::Ender::Coordinator,
    };
    let item = |id| ItemWait::Item {
        id,
        title: "Other".into(),
        state: "ready",
    };
    let answer = ItemWait::Answer {
        id: 438,
        title: "Which port?".into(),
    };
    assert_eq!(
        ranked([
            condition.clone(),
            item(2),
            answer.clone(),
            condition.clone(),
            item(1),
            item(2),
        ]),
        [answer, item(2), item(1), condition]
    );
}

#[test]
fn a_branch_with_an_app_build_waits_on_it_and_is_not_ready_to_land() {
    let card = words(
        &card(Some("working"), claim(None), Some(0)),
        st("working"),
        &[],
        &[ItemWait::AppBuild {
            lane: Lane::Mac,
            running: false,
            how: "place 2 in the queue".into(),
            queued: None,
        }],
    );
    assert_eq!(card.chip, Some("Working".to_string()));
    assert_eq!(card.pill, None, "the build has not started");
    assert_eq!(
        card.note.as_deref(),
        Some("Waits on the Mac app build: place 2 in the queue")
    );
}

#[test]
fn a_branch_whose_app_build_runs_names_the_lane_in_the_pill() {
    let card = waiting(
        "working",
        ItemWait::AppBuild {
            lane: Lane::Android,
            running: true,
            how: "builds in side-1 for 1 minute".into(),
            queued: None,
        },
    );
    assert_eq!(
        card.pill,
        pill(
            "Android build",
            "Android app build: builds in side-1 for 1 minute"
        )
    );
}

#[test]
fn ready_to_land_needs_no_active_shell_no_app_build_and_no_waiting_item() {
    let build = ItemWait::AppBuild {
        lane: Lane::Mac,
        running: true,
        how: "builds in side-1 for 1 minute".into(),
        queued: None,
    };
    let item = ItemWait::Item {
        id: 1,
        title: "Other".into(),
        state: "ready",
    };
    assert!(!keeps_working(CardState::ReadyToLand, false, None));
    assert!(keeps_working(CardState::ReadyToLand, true, None));
    assert!(keeps_working(CardState::ReadyToLand, false, Some(&build)));
    assert!(keeps_working(CardState::ReadyToLand, false, Some(&item)));
    let landing = ItemWait::Landing {
        id: 2,
        title: "Heading row".into(),
    };
    assert!(keeps_working(CardState::ReadyToLand, false, Some(&landing)));
    assert!(!keeps_working(
        CardState::ReadyToLand,
        false,
        Some(&ItemWait::Unrecorded)
    ));
    assert!(!keeps_working(CardState::Working, true, None));
}

#[test]
fn a_worktree_whose_waits_the_daemon_cannot_read_says_so_and_never_working_or_ready_to_land() {
    for state in [
        CardState::ReadyToLand,
        CardState::Working,
        CardState::Paused,
        CardState::WaitingOnYou,
    ] {
        assert_eq!(
            card_state(state, true, None, None),
            CardState::WaitsUnread,
            "{state:?}"
        );
    }
    let question = ItemWait::Answer {
        id: 1,
        title: "Choose a name".into(),
    };
    assert_eq!(
        card_state(CardState::WaitsUnread, false, Some(&[question]), None),
        CardState::WaitsUnread,
    );
    let card = words(
        &card(Some("waits-unread"), claim(Some("note")), Some(0)),
        st("waits-unread"),
        &running(&[(7, "Reopen a copied library")]),
        &[],
    );
    assert_eq!(
        card.chip,
        Some("Can't read what this worktree waits on".to_string())
    );
    assert_eq!(card.pill, None);
    assert_eq!(card.note, None);
}

#[test]
fn a_worktree_whose_waits_the_daemon_read_takes_its_state_from_the_first() {
    let item = ItemWait::Item {
        id: 1,
        title: "Other".into(),
        state: "ready",
    };
    let answer = ItemWait::Answer {
        id: 438,
        title: "Which port?".into(),
    };
    let state = |state, waits: &[ItemWait]| card_state(state, false, Some(waits), None);
    assert_eq!(state(CardState::ReadyToLand, &[]), CardState::ReadyToLand);
    assert_eq!(
        state(CardState::ReadyToLand, std::slice::from_ref(&item)),
        CardState::Working
    );
    assert_eq!(
        state(
            CardState::ReadyToLand,
            &[ItemWait::RulesReview(ReviewWait::ToSettle(
                2,
                Settler::Judge
            ))]
        ),
        CardState::InReview(ReviewWait::ToSettle(2, Settler::Judge))
    );
    assert_eq!(
        state(CardState::Working, &[queued_wait(false)]),
        CardState::Queued(Lane::Mac)
    );
    assert_eq!(
        state(CardState::Working, &[answer, item]),
        CardState::WaitingOnYou
    );
}

#[test]
fn a_worktree_an_item_keeps_takes_its_state_from_the_items_waits() {
    assert_eq!(
        card_state(
            CardState::ReadyToLand,
            false,
            None,
            Some(Ok(&KeptCard::None))
        ),
        CardState::Kept
    );
    assert_eq!(
        card_state(CardState::Free, false, Some(&[]), Some(Err(()))),
        CardState::WaitsUnread
    );
}

fn queued_wait(running: bool) -> ItemWait {
    ItemWait::AppBuild {
        lane: Lane::Mac,
        running,
        how: "place 1 in the queue".into(),
        queued: (!running).then(|| QueuedBuild {
            place: 1,
            of: 4,
            line: "Its agent asked 8m ago to run macos macos-tests.".into(),
            steps: Some("macos macos-tests".into()),
        }),
    }
}

#[test]
fn a_worktree_whose_app_build_has_not_started_is_queued_and_one_whose_build_runs_is_working() {
    assert_eq!(
        waits_in_queue(CardState::Working, Some(&queued_wait(false))),
        Some(Lane::Mac)
    );
    assert_eq!(
        waits_in_queue(CardState::Working, Some(&queued_wait(true))),
        None
    );
    assert_eq!(
        waits_in_queue(CardState::WaitingOnYou, Some(&queued_wait(false))),
        None
    );
    assert_eq!(waits_in_queue(CardState::Working, None), None);
}

#[test]
fn a_queued_card_names_the_lane_and_carries_its_place_and_what_it_asked() {
    let card = words(
        &card(Some("queued"), claim(None), Some(0)),
        st("queued"),
        &[],
        &[queued_wait(false)],
    );
    assert_eq!(card.chip, Some("Waiting on Mac".to_string()));
    assert_eq!(card.pill, None, "the chip says it");
    assert_eq!(
        card.note.as_deref(),
        Some("Its agent asked 8m ago to run macos macos-tests.")
    );
    let queued = card.queued.unwrap();
    assert_eq!((queued.place, queued.of), (1, 4));
    assert_eq!(queued.steps.as_deref(), Some("macos macos-tests"));
}

#[test]
fn commits_whose_rules_review_is_not_finished_are_in_review_and_the_card_says_where_it_stands() {
    let review = |wait| ItemWait::RulesReview(wait);
    let state = |worktree, waits: &[ItemWait]| card_state(worktree, false, Some(waits), None);
    assert_eq!(
        state(CardState::ReadyToLand, &[review(ReviewWait::NotRun)]),
        CardState::InReview(ReviewWait::NotRun)
    );
    assert_eq!(state(CardState::ReadyToLand, &[]), CardState::ReadyToLand);
    assert_eq!(
        state(CardState::Working, &[review(ReviewWait::NotRun)]),
        CardState::Working
    );
    assert_eq!(
        state(
            CardState::ReadyToLand,
            &[ItemWait::RulesReview(ReviewWait::DesignCoverage)]
        ),
        CardState::InReview(ReviewWait::DesignCoverage)
    );
    let said = |wait| {
        let card = words(
            &card(Some("in-review"), claim(None), Some(0)),
            CardState::InReview(wait),
            &running(&[(117, "Item")]),
            &[review(wait)],
        );
        (card.chip.unwrap(), card.pill)
    };
    assert_eq!(said(ReviewWait::NotRun), ("Needs review".to_string(), None));
    assert_eq!(said(ReviewWait::Running), ("Reviewing".to_string(), None));
    assert_eq!(
        said(ReviewWait::ToSettle(3, Settler::Coordinator)),
        (
            "Findings to settle".to_string(),
            pill(
                "3 findings",
                "3 review findings wait for the coordinator to settle them."
            )
        )
    );
    assert_eq!(
        said(ReviewWait::ToSettle(1, Settler::Judge)),
        (
            "Findings to settle".to_string(),
            pill(
                "1 finding",
                "1 review finding waits for the judge to settle it."
            )
        )
    );
    assert_eq!(
        said(ReviewWait::Open(1)),
        (
            "Open findings".to_string(),
            pill(
                "1 finding",
                "1 open confirmed review finding waits for a review that closes it."
            )
        )
    );
    assert_eq!(
        said(ReviewWait::Unreviewed(2)),
        (
            "Unreviewed changes".to_string(),
            pill(
                "2 files",
                "2 files changed since the last review: builds review reads the changes in a new run."
            )
        )
    );
}

/// The card of a worktree an item keeps says what the item waits on.
#[test]
fn a_worktree_an_item_keeps_says_what_its_item_waits_on() {
    let item = RunningItem {
        id: 7,
        title: "Do the kept thing now".into(),
    };
    let page = card(Some("kept"), None, Some(2));
    let said = kept_words(&page, item.clone(), &KeptCard::None);
    assert_eq!(
        (said.chip, said.items.clone(), said.note),
        (Some("Ready".to_string()), vec![item.clone()], None)
    );
    let open = [
        ItemWait::Item {
            id: 793,
            title: "Keep the tool running".into(),
            state: "ready",
        },
        ItemWait::Answer {
            id: 816,
            title: "Which way".into(),
        },
    ];
    let open = KeptCard::Open(open.to_vec());
    assert_eq!(kept_state(&open), CardState::KeptWaiting);
    let said = kept_words(&page, item.clone(), &open);
    assert_eq!(said.chip, Some("Waiting".to_string()));
    assert_eq!(
        said.note.as_deref(),
        Some(
            "Waits on #793 (ready): Keep the tool running. Waits on your answer to #816: Which way"
        )
    );
    let met = KeptCard::Met(vec![
        "question #816 answered by decision #9: Which way".into(),
    ]);
    assert_eq!(kept_state(&met), CardState::KeptMet);
    let said = kept_words(&page, item, &met);
    assert_eq!(said.chip, Some("Waits met".to_string()));
    assert_eq!(
        said.note.as_deref(),
        Some(
            "Every wait is met: question #816 answered by decision #9: Which way. Land its commits, or resume its agent with builds item take."
        )
    );
}
