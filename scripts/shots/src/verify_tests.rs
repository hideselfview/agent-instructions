use super::*;

fn name(text: &str) -> Name {
    Name::parse(text).unwrap()
}

fn outcome(checked: Vec<Checked>) -> Outcome {
    Outcome {
        runtime: String::new(),
        checked,
        problems: Vec::new(),
        lists: Lists::default(),
        not_taken: Vec::new(),
        failed: Vec::new(),
        components: crate::component_files::Listing::default(),
    }
}

#[test]
fn a_run_passes_without_findings_or_problems() {
    let mut outcome = outcome(Vec::new());
    outcome.components = crate::component_files::Listing {
        components: Vec::new(),
        strays: vec![crate::component_files::Stray {
            file: "app/Shots/Components/MacGone.swift".into(),
            stem: "MacGone".to_string(),
            why: "no preview".to_string(),
        }],
        unnamed: Vec::new(),
    };
    assert!(
        outcome.passed(),
        "a capture file with no preview fails nothing"
    );
    outcome.problems.push(Problem {
        name: "x".into(),
        sentence: "s".into(),
    });
    assert!(!outcome.passed());
}

/// A pair with one finding, standing <against> the before run.
fn pair_with_finding(against: Option<Against>) -> Checked {
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
    Checked {
        name: name("mac.welcome.a"),
        board: list.clone(),
        shot: list,
        pairs: Vec::new(),
        offset: (0.0, 0.0),
        board_png: PathBuf::new(),
        shot_png: PathBuf::new(),
        findings: vec![finding],
        warnings: Vec::new(),
        against,
    }
}

#[test]
fn a_pair_with_a_finding_fails_a_run_with_no_before_run() {
    let outcome = outcome(vec![pair_with_finding(None)]);
    assert!(!outcome.passed());
    assert_eq!(outcome.failing().count(), 1);
}

#[test]
fn a_new_or_worse_pair_with_a_finding_fails_the_run() {
    for against in [Against::New, Against::Worse] {
        let outcome = outcome(vec![pair_with_finding(Some(against))]);
        assert!(!outcome.passed(), "{against:?}");
        assert_eq!(outcome.failing().count(), 1);
    }
}

#[test]
fn a_pair_that_holds_only_the_before_runs_findings_is_only_listed() {
    let outcome = outcome(vec![pair_with_finding(Some(Against::Held))]);
    assert!(outcome.passed());
    assert_eq!(outcome.listed().count(), 1);
}

#[test]
fn a_new_pair_without_findings_passes() {
    let mut pair = pair_with_finding(Some(Against::New));
    pair.findings.clear();
    assert!(outcome(vec![pair]).passed());
}

#[test]
fn a_pair_the_base_never_rendered_lists_its_findings_and_fails_nothing() {
    let mut without = pair_with_finding(Some(Against::Unrendered));
    without.findings.clear();
    let outcome = outcome(vec![pair_with_finding(Some(Against::Unrendered)), without]);
    assert!(outcome.passed());
    assert_eq!(outcome.failing().count(), 0);
    assert_eq!(outcome.listed().count(), 1);
    assert_eq!(outcome.unrendered().count(), 2);
}

#[test]
fn the_run_of_the_merge_base_lists_every_finding_and_fails_no_pair() {
    let outcome = outcome(vec![
        pair_with_finding(Some(Against::Base)),
        pair_with_finding(Some(Against::Base)),
    ]);
    assert!(
        outcome.passed(),
        "a run with no base fails no pair for lack of one"
    );
    assert_eq!(outcome.failing().count(), 0);
    assert_eq!(outcome.listed().count(), 2);
    assert_eq!(mark(Some(Against::Base)), " (the merge base's own run)");
    let without = pair_with_finding(None);
    assert!(
        !outcome_of(without).passed(),
        "a run with no base and no --base fails each pair with a finding"
    );
}

fn outcome_of(pair: Checked) -> Outcome {
    outcome(vec![pair])
}

#[test]
fn a_problem_fails_a_run_whose_pairs_hold() {
    let mut outcome = outcome(vec![pair_with_finding(Some(Against::Held))]);
    outcome.problems.push(Problem {
        name: "x".into(),
        sentence: "s".into(),
    });
    assert!(!outcome.passed());
}

#[test]
fn a_line_names_how_its_pair_stands_against_the_base() {
    assert_eq!(mark(Some(Against::New)), " (new against the base)");
    assert_eq!(mark(Some(Against::Worse)), " (worse than the base)");
    assert_eq!(
        mark(Some(Against::Unrendered)),
        " (the base never rendered it)"
    );
    assert_eq!(mark(Some(Against::Held)), "");
    assert_eq!(mark(None), "");
}

#[test]
fn the_run_draws_only_the_boards_the_picture_folder_lacks() {
    let dir = tempfile::tempdir().unwrap();
    let names = [name("mac.a.b"), name("mac.a.c")];
    let hashes: BTreeMap<Name, String> = [
        (names[0].clone(), "kept".to_string()),
        (names[1].clone(), "new".to_string()),
    ]
    .into_iter()
    .collect();
    let boards = dir.path().join("boards");
    assert_eq!(
        take_pictures(None, &hashes, &names, &boards).unwrap(),
        names.to_vec()
    );
    let drawn = dir.path().join("drawn");
    std::fs::create_dir_all(&drawn).unwrap();
    std::fs::write(drawn.join("mac.a.b.png"), "png").unwrap();
    std::fs::write(drawn.join("mac.a.b.json"), "{}").unwrap();
    let pictures = Pictures::new(dir.path().join("pictures"), std::time::SystemTime::now());
    pictures.keep_board("kept", "mac.a.b", &drawn).unwrap();
    assert_eq!(
        take_pictures(Some(&pictures), &hashes, &names, &boards).unwrap(),
        [names[1].clone()]
    );
    assert!(boards.join("mac.a.b.png").is_file() && boards.join("mac.a.b.json").is_file());
}
