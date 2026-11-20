use super::*;

fn name(text: &str) -> Name {
    Name::parse(text).unwrap()
}

fn hashes(pairs: &[(&str, &str)]) -> BTreeMap<Name, String> {
    pairs
        .iter()
        .map(|(n, h)| (name(n), h.to_string()))
        .collect()
}

fn record_with(board: &str, board_hash: &str, shot_hash: &str) -> Record {
    let mut record = Record::default();
    record.set(
        board,
        Entry {
            board: board_hash.into(),
            shot: shot_hash.into(),
            findings: Vec::new(),
            warnings: Vec::new(),
        },
    );
    record
}

#[test]
fn a_pair_whose_two_hashes_equal_its_entry_is_skipped() {
    let names = [name("mac.welcome.a")];
    let record = record_with("mac.welcome.a", "b", "s");
    let (skipped, pending) = partition(
        &names,
        &hashes(&[("mac.welcome.a", "b")]),
        &hashes(&[("mac.welcome.a", "s")]),
        &record,
    );
    assert_eq!((skipped.len(), pending.len()), (1, 0));
}

#[test]
fn a_pair_is_rechecked_when_either_hash_changed() {
    let names = [name("mac.welcome.a")];
    let record = record_with("mac.welcome.a", "b", "s");
    for (board, shot) in [("b2", "s"), ("b", "s2")] {
        let (skipped, pending) = partition(
            &names,
            &hashes(&[("mac.welcome.a", board)]),
            &hashes(&[("mac.welcome.a", shot)]),
            &record,
        );
        assert_eq!((skipped.len(), pending.len()), (0, 1), "{board} {shot}");
    }
}

#[test]
fn a_pair_with_no_entry_is_checked() {
    let names = [name("mac.welcome.a")];
    let (skipped, pending) = partition(
        &names,
        &hashes(&[("mac.welcome.a", "b")]),
        &hashes(&[("mac.welcome.a", "s")]),
        &Record::default(),
    );
    assert_eq!((skipped.len(), pending.len()), (0, 1));
}

#[test]
fn a_pair_is_new_against_a_base_that_lacks_it_and_changed_against_one_that_differs() {
    let base = record_with("mac.welcome.a", "b", "s");
    assert_eq!(
        base.standing("mac.welcome.a", "b", "s"),
        Standing::Unchanged
    );
    assert_eq!(base.standing("mac.welcome.a", "b2", "s"), Standing::Changed);
    assert_eq!(base.standing("mac.welcome.b", "b", "s"), Standing::New);
}

#[test]
fn a_run_passes_without_findings_or_problems() {
    let mut outcome = Outcome {
        runtime: String::new(),
        checked: Vec::new(),
        skipped: Vec::new(),
        problems: Vec::new(),
        lists: Lists::default(),
        not_taken: Vec::new(),
        components: crate::component_files::Listing {
            components: Vec::new(),
            strays: vec![crate::component_files::Stray {
                file: "app/Shots/Components/MacGone.swift".into(),
                why: "no preview".to_string(),
            }],
        },
        gate: Gate::EveryPair,
        base_empty: false,
    };
    assert!(
        outcome.passed(),
        "a capture file with no preview fails nothing"
    );
    outcome.problems.push(Problem {
        name: "x".into(),
        declared: false,
        sentence: "s".into(),
    });
    assert!(!outcome.passed());
}

fn outcome_with_finding(gate: Gate, base_empty: bool, against_base: Option<Standing>) -> Outcome {
    let finding = Finding::Missing(crate::compare::normalize::Entry {
        index: 0,
        folded: Vec::new(),
        kind: crate::shots::Role::Text,
        words: "x".into(),
        value: String::new(),
        enabled: true,
        selected: false,
    });
    let list = crate::shots::ElementList::parse(crate::shots::tests::LIST).unwrap();
    Outcome {
        runtime: String::new(),
        checked: vec![Checked {
            name: name("mac.welcome.a"),
            board: list.clone(),
            shot: list,
            pairs: Vec::new(),
            offset: (0.0, 0.0),
            board_png: PathBuf::new(),
            shot_png: PathBuf::new(),
            findings: vec![finding],
            warnings: Vec::new(),
            against_base,
        }],
        skipped: Vec::new(),
        problems: Vec::new(),
        lists: Lists::default(),
        not_taken: Vec::new(),
        components: crate::component_files::Listing::default(),
        gate,
        base_empty,
    }
}

#[test]
fn a_pair_with_a_finding_fails_a_run_that_gates_every_pair() {
    for standing in [None, Some(Standing::Unchanged), Some(Standing::New)] {
        assert!(!outcome_with_finding(Gate::EveryPair, false, standing).passed());
    }
}

#[test]
fn a_new_or_changed_pair_with_a_finding_fails_a_local_run() {
    for standing in [Standing::New, Standing::Changed] {
        let outcome = outcome_with_finding(Gate::NewOrChanged, false, Some(standing));
        assert!(!outcome.passed());
        assert_eq!(outcome.failing().count(), 1);
    }
}

#[test]
fn a_pair_confirmed_against_the_base_with_a_finding_is_only_listed_by_a_local_run() {
    let outcome = outcome_with_finding(Gate::NewOrChanged, false, Some(Standing::Unchanged));
    assert!(outcome.passed());
    assert_eq!(outcome.listed().count(), 1);
}

#[test]
fn a_base_with_no_record_gates_no_pair_of_a_local_run() {
    let outcome = outcome_with_finding(Gate::NewOrChanged, true, None);
    assert!(outcome.passed());
    assert_eq!(outcome.listed().count(), 1);
}

#[test]
fn a_problem_fails_a_local_run() {
    let mut outcome = outcome_with_finding(Gate::NewOrChanged, false, Some(Standing::Unchanged));
    outcome.problems.push(Problem {
        name: "x".into(),
        declared: true,
        sentence: "s".into(),
    });
    assert!(!outcome.passed());
}

#[test]
fn an_empty_record_has_no_entries() {
    assert!(Record::default().is_empty());
    assert!(!record_with("mac.welcome.a", "b", "s").is_empty());
}

fn skipped_with_finding(against_base: Option<Standing>) -> Skipped {
    Skipped {
        name: name("mac.welcome.a"),
        findings: vec!["Shot is missing the button “Set up”".to_string()],
        warnings: Vec::new(),
        against_base,
    }
}

#[test]
fn a_skipped_pair_with_a_recorded_finding_fails_a_run_that_gates_every_pair() {
    let mut outcome = outcome_with_finding(Gate::EveryPair, false, None);
    outcome.checked.clear();
    assert!(outcome.passed());
    outcome.skipped.push(skipped_with_finding(None));
    assert!(!outcome.passed());
    assert_eq!(outcome.failing_skipped().count(), 1);
}

#[test]
fn a_skipped_pair_unchanged_against_the_base_is_only_listed_by_a_local_run() {
    let mut outcome = outcome_with_finding(Gate::NewOrChanged, false, None);
    outcome.checked.clear();
    outcome
        .skipped
        .push(skipped_with_finding(Some(Standing::Unchanged)));
    assert!(outcome.passed());
    assert_eq!(outcome.listed_skipped().count(), 1);
    outcome.skipped[0].against_base = Some(Standing::Changed);
    assert!(!outcome.passed());
}
