use super::*;

fn keys(words: &str) -> Vec<(Role, String)> {
    words
        .split(' ')
        .map(|w| (Role::Text, w.to_string()))
        .collect()
}

fn steps_of(board: &str, shot: &str) -> Vec<Step> {
    align(&keys(board), &keys(shot))
}

#[test]
fn equal_lists_match_one_to_one() {
    assert_eq!(
        steps_of("a b", "a b"),
        [Step::Match(0, 0), Step::Match(1, 1)]
    );
}

#[test]
fn a_board_element_left_over_is_a_board_step_and_a_shot_one_a_shot_step() {
    assert_eq!(
        steps_of("a b c", "a c d"),
        [
            Step::Match(0, 0),
            Step::Board(1),
            Step::Match(2, 1),
            Step::Shot(2)
        ]
    );
}

#[test]
fn the_longest_common_subsequence_is_kept() {
    // b c d survive, a is the swapped one.
    let steps = steps_of("a b c d", "b c d a");
    let matched = steps
        .iter()
        .filter(|s| matches!(s, Step::Match(..)))
        .count();
    assert_eq!(matched, 3);
}

#[test]
fn a_kind_is_part_of_the_key() {
    let board = vec![(Role::Button, "Go".to_string())];
    let shot = vec![(Role::Text, "Go".to_string())];
    assert_eq!(align(&board, &shot), [Step::Board(0), Step::Shot(0)]);
}

#[test]
fn empty_lists_align_to_nothing() {
    assert!(align(&[], &[]).is_empty());
}
