use super::*;
use crate::claim::Claim;
use crate::daemon::model::Activity;
use crate::side_git::SideGit;

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
        git: Some(SideGit {
            head: None,
            ahead: Vec::new(),
            uncommitted,
            last_write: None,
            error: None,
        }),
        state,
        folders: None,
        total_kib: None,
        activity: Activity::default(),
        words: Words::default(),
        stalled: false,
        errors: Vec::new(),
    }
}

fn claim(note: Option<&str>) -> Option<Claim> {
    Some(Claim {
        who: "agent".into(),
        since: 1_790_000_000,
        note: note.map(str::to_string),
        note_state: None,
    })
}

#[test]
fn the_chip_names_only_a_worktree_with_an_agent_or_commits() {
    let chip = |state| words(&card(Some(state), claim(None), Some(0)), &[], &[]).chip;
    assert_eq!(chip("ready-to-land"), Some("Ready to land"));
    assert_eq!(chip("working"), Some("Working"));
    assert_eq!(chip("held"), Some("Paused"));
    assert_eq!(chip("winding-down"), Some("Working"));
    assert_eq!(chip("waiting-on-you"), Some("Waiting on you"));
    assert_eq!(chip("free"), None);
    let mut main = card(None, None, None);
    main.main = true;
    assert_eq!(words(&main, &[], &[]).chip, None);
}

#[test]
fn with_no_running_item_the_card_shows_the_agents_note_or_what_the_state_means() {
    let note = |state, note| words(&card(Some(state), claim(note), Some(0)), &[], &[]).note;
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
        Some("Waits for the coordinator to land it.")
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
        words(&card(Some("ready-to-land"), None, Some(0)), &[], &[])
            .note
            .as_deref(),
        Some("Commits wait to land and no agent claims this worktree.")
    );
    let mut main = card(None, None, None);
    main.main = true;
    assert_eq!(
        words(&main, &[], &[]).note.as_deref(),
        Some("Your checkout. No agent works or builds here.")
    );
}

#[test]
fn the_footer_counts_uncommitted_files_and_says_nothing_for_a_clean_tree() {
    let files = |n| words(&card(Some("working"), claim(None), n), &[], &[]).uncommitted;
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
    broken.git.as_mut().unwrap().error = Some("git failed".into());
    assert_eq!(words(&broken, &[], &[]).uncommitted, None);
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
    assert_eq!(card.chip, Some("Waiting on you"));
    assert_eq!(
        card.pill,
        pill(
            "Answer #438",
            "Waits on your answer: When to reopen a copied library"
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
    assert_eq!(card.chip, Some("Working"));
    assert_eq!(
        card.pill,
        pill("After #1320", "Waits on #1320: Store the library id")
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
    assert_eq!(card.chip, Some("Working"));
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
        &[],
        &[ItemWait::Unrecorded],
    );
    assert_eq!(card.chip, Some("Ready to land"));
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
        &[],
        &[answer],
    );
    assert!(asked.pill.is_some());
    assert_eq!(asked.note, None);
    let unrecorded = words(
        &card(Some("ready-to-land"), claim(None), Some(0)),
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
        ItemWait::Condition("builds #122, #121 and #125 land".into()),
    ];
    let card = words(
        &card(Some("held"), claim(Some("Until the store lands.")), Some(0)),
        &running(&[(3, "Item")]),
        &waits,
    );
    assert_eq!(card.chip, Some("Paused"));
    assert_eq!(card.pill, None);
    assert_eq!(card.items, running(&[(3, "Item")]));
    assert_eq!(
        card.note.as_deref(),
        Some("Waits on #1320: Store the library id. Waits until builds #122, #121 and #125 land")
    );
    let noted = listed("held", Some("Until the store lands."), &[(3, "Item")]);
    assert_eq!(noted.chip, Some("Paused"));
    assert_eq!(noted.note.as_deref(), Some("Until the store lands."));
    let unnoted = listed("held", None, &[]);
    assert_eq!(
        unnoted.note.as_deref(),
        Some("Its agent holds this worktree on purpose and runs no work.")
    );
}

#[test]
fn a_condition_ranks_after_every_other_wait_and_has_no_pill() {
    let condition = ItemWait::Condition("builds #122 lands".into());
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
    let condition = ItemWait::Condition("builds #122 lands".into());
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
        &[],
        &[ItemWait::AppBuild {
            lane: "Mac".into(),
            running: false,
            how: "place 2 in the queue".into(),
            queued: None,
        }],
    );
    assert_eq!(card.chip, Some("Working"));
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
            lane: "Android".into(),
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
        lane: "Mac".into(),
        running: true,
        how: "builds in side-1 for 1 minute".into(),
        queued: None,
    };
    let item = ItemWait::Item {
        id: 1,
        title: "Other".into(),
        state: "ready",
    };
    assert!(!keeps_working(Some("ready-to-land"), false, None));
    assert!(keeps_working(Some("ready-to-land"), true, None));
    assert!(keeps_working(Some("ready-to-land"), false, Some(&build)));
    assert!(keeps_working(Some("ready-to-land"), false, Some(&item)));
    let landing = ItemWait::Landing {
        id: 2,
        title: "Heading row".into(),
    };
    assert!(keeps_working(Some("ready-to-land"), false, Some(&landing)));
    assert!(!keeps_working(
        Some("ready-to-land"),
        false,
        Some(&ItemWait::Unrecorded)
    ));
    assert!(!keeps_working(Some("working"), true, None));
}

#[test]
fn a_worktree_whose_waits_the_daemon_cannot_read_says_so_and_never_working_or_ready_to_land() {
    for state in ["ready-to-land", "working", "held", "waiting-on-you"] {
        assert_eq!(
            card_state(Some(state), true, None),
            Some("waits-unread"),
            "{state}"
        );
    }
    assert_eq!(card_state(None, false, None), None, "a main checkout");
    let card = words(
        &card(Some("waits-unread"), claim(Some("note")), Some(0)),
        &running(&[(7, "Reopen a copied library")]),
        &[],
    );
    assert_eq!(card.chip, Some("Can't read what this worktree waits on"));
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
    let state = |state, waits: &[ItemWait]| card_state(Some(state), false, Some(waits));
    assert_eq!(state("ready-to-land", &[]), Some("ready-to-land"));
    assert_eq!(
        state("ready-to-land", std::slice::from_ref(&item)),
        Some("working")
    );
    assert_eq!(
        state(
            "ready-to-land",
            &[ItemWait::RulesReview(ReviewWait::NotRun)]
        ),
        Some("in-review")
    );
    assert_eq!(state("working", &[queued_wait(false)]), Some("queued"));
    assert_eq!(state("working", &[answer, item]), Some("waiting-on-you"));
    assert_eq!(card_state(None, false, Some(&[])), None, "a main checkout");
}

fn queued_wait(running: bool) -> ItemWait {
    ItemWait::AppBuild {
        lane: "Mac".into(),
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
    assert!(waits_in_queue(Some("working"), Some(&queued_wait(false))));
    assert!(!waits_in_queue(Some("working"), Some(&queued_wait(true))));
    assert!(!waits_in_queue(
        Some("waiting-on-you"),
        Some(&queued_wait(false))
    ));
    assert!(!waits_in_queue(Some("working"), None));
}

#[test]
fn a_queued_card_names_the_lane_and_carries_its_place_and_what_it_asked() {
    let card = words(
        &card(Some("queued"), claim(None), Some(0)),
        &[],
        &[queued_wait(false)],
    );
    assert_eq!(card.chip, Some("Waiting on Mac"));
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
    assert!(in_review(
        Some("ready-to-land"),
        Some(&review(ReviewWait::NotRun))
    ));
    assert!(!in_review(Some("ready-to-land"), None));
    assert!(!in_review(
        Some("working"),
        Some(&review(ReviewWait::NotRun))
    ));
    let said = |wait| {
        let card = words(
            &card(Some("in-review"), claim(None), Some(0)),
            &running(&[(117, "Item")]),
            &[review(wait)],
        );
        (card.chip.unwrap(), card.pill)
    };
    assert_eq!(said(ReviewWait::NotRun), ("Needs review", None));
    assert_eq!(said(ReviewWait::Running), ("Reviewing", None));
    assert_eq!(
        said(ReviewWait::ToSettle(3)),
        (
            "Findings to settle",
            pill(
                "3 findings",
                "3 review findings wait on the coordinator to settle them."
            )
        )
    );
    assert_eq!(
        said(ReviewWait::Open(1)),
        (
            "Open findings",
            pill(
                "1 finding",
                "1 open confirmed review finding waits for a review that closes it."
            )
        )
    );
    assert_eq!(
        said(ReviewWait::Unreviewed(2)),
        (
            "Unreviewed changes",
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
    let said = kept_words(&page, item.clone(), &KeptWaits::None, &[]);
    assert_eq!(
        (said.chip, said.items.clone(), said.note),
        (Some("Ready"), vec![item.clone()], None)
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
    assert_eq!(kept_state(&KeptWaits::Open), "kept-waiting");
    let said = kept_words(&page, item.clone(), &KeptWaits::Open, &open);
    assert_eq!(said.chip, Some("Waiting"));
    assert_eq!(
        said.note.as_deref(),
        Some(
            "Waits on item #793 (ready): Keep the tool running; question #816 (waits on you): Which way."
        )
    );
    let met = KeptWaits::Met(vec![
        "question #816 answered by decision #9: Which way".into(),
    ]);
    assert_eq!(kept_state(&met), "kept-met");
    let said = kept_words(&page, item, &met, &[]);
    assert_eq!(said.chip, Some("Waits met"));
    assert_eq!(
        said.note.as_deref(),
        Some(
            "Every wait is met: question #816 answered by decision #9: Which way. Land its commits, or resume its agent with builds item take."
        )
    );
}
