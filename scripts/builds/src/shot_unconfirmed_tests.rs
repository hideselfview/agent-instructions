use super::*;
use serde_json::json;

/// A run that checked the scenes of <boards> (scene, board hash) and has the
/// findings <findings> (scene, sentence).
fn run(boards: &[(&str, &str)], findings: &[(&str, &str)]) -> Run {
    let scenes: Vec<&str> = boards.iter().map(|(scene, _)| *scene).collect();
    let boards: serde_json::Map<String, serde_json::Value> = boards
        .iter()
        .map(|(scene, hash)| (scene.to_string(), json!(hash)))
        .collect();
    let findings: Vec<_> = findings
        .iter()
        .map(|(scene, sentence)| {
            json!({"scene": scene, "level": "warning", "kind": "region", "sentence": sentence})
        })
        .collect();
    shot_causes::parse(
        &json!({"scenes": scenes, "boards": boards, "findings": findings}).to_string(),
    )
    .unwrap()
}

fn others(run: &Run) -> Vec<Option<(usize, usize)>> {
    run.findings
        .iter()
        .map(|r| r.others.map(|o| (o.of, o.without)))
        .collect()
}

#[test]
fn a_finding_that_another_run_of_the_same_board_lacks_is_unconfirmed() {
    let mut mine = run(&[("mac.a", "h1")], &[("mac.a", "grey")]);
    let shows = run(&[("mac.a", "h1")], &[("mac.a", "grey")]);
    let lacks = run(&[("mac.a", "h1")], &[]);
    apply(&mut mine, &[shows, lacks]);
    assert_eq!(others(&mine), vec![Some((2, 1))]);
    assert!(mine.findings[0].unconfirmed());
}

#[test]
fn a_finding_every_other_run_shows_is_confirmed() {
    let mut mine = run(&[("mac.a", "h1")], &[("mac.a", "grey")]);
    let shows = run(&[("mac.a", "h1")], &[("mac.a", "grey")]);
    apply(&mut mine, &[shows]);
    assert_eq!(others(&mine), vec![Some((1, 0))]);
    assert!(!mine.findings[0].unconfirmed());
}

#[test]
fn a_run_of_another_board_says_nothing_of_the_scene() {
    let mut mine = run(&[("mac.a", "h1")], &[("mac.a", "grey")]);
    let other_board = run(&[("mac.a", "h2")], &[]);
    let other_scene = run(&[("mac.b", "h1")], &[]);
    apply(&mut mine, &[other_board, other_scene]);
    assert_eq!(others(&mine), vec![None]);
}

#[test]
fn each_scene_counts_only_the_runs_that_checked_it_with_its_board() {
    let mut mine = run(
        &[("mac.a", "h1"), ("mac.b", "h2")],
        &[("mac.a", "grey"), ("mac.b", "red")],
    );
    let both = run(&[("mac.a", "h1"), ("mac.b", "h2")], &[("mac.b", "red")]);
    let only_a = run(&[("mac.a", "h1"), ("mac.b", "h9")], &[]);
    apply(&mut mine, &[both, only_a]);
    assert_eq!(others(&mine), vec![Some((2, 2)), Some((1, 0))]);
}

#[test]
fn a_run_that_records_no_board_hashes_marks_nothing() {
    let mut mine = run(&[], &[("mac.a", "grey")]);
    let theirs = run(&[("mac.a", "h1")], &[]);
    apply(&mut mine, &[theirs]);
    assert_eq!(others(&mine), vec![None]);
}

#[test]
fn the_plain_text_marks_an_unconfirmed_finding_and_the_causes_leave_it_out() {
    let mut mine = run(
        &[("mac.a", "h1"), ("mac.b", "h1")],
        &[("mac.a", "grey"), ("mac.b", "red")],
    );
    let theirs = run(&[("mac.a", "h1")], &[]);
    apply(&mut mine, &[theirs]);
    let lines = shot_causes::lines(&mine);
    assert!(
        lines.contains(
            "mac.a: warning: grey [unconfirmed: 1 of 1 other run of this scene with the same board and app code don't show it]\n"
        ),
        "{lines}"
    );
    assert!(lines.contains("mac.b: warning: red\n"), "{lines}");
    assert!(
        lines.contains("2 findings in 2 of the 2 scenes the run checked, 1 of them unconfirmed"),
        "{lines}"
    );
    let causes = shot_causes::text(&mine);
    assert!(
        causes.contains("Unconfirmed, in no cause above"),
        "{causes}"
    );
    assert!(causes.contains("mac.a (1)"), "{causes}");
    assert_eq!(shot_causes::causes(&mine).remainder.len(), 1);
}
