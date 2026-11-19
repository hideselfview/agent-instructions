use super::*;
use crate::lists::fixtures::{NOON, db, decision_answering};
use crate::review::mechanical::Mechanical;

const CHANGE: Change = Change {
    repo: "bae",
    key: "work",
};

fn stamp() -> Stamp {
    Stamp {
        at: NOON,
        by: "tester".to_string(),
    }
}

fn branch(patch_id: &str) -> Branch<'_> {
    Branch {
        change: CHANGE,
        patch_id,
    }
}

const ONE: &str = "1111111111111111111111111111111111111111";
const TWO: &str = "2222222222222222222222222222222222222222";

#[test]
fn a_branch_nobody_recorded_has_no_standing_and_a_mechanical_one_needs_no_review() {
    let db = db();
    assert_eq!(standing(&db, branch(ONE)).unwrap(), None);
    record_mechanical(&db, branch(ONE), Mechanical::Rename, &stamp()).unwrap();
    assert_eq!(
        standing(&db, branch(ONE)).unwrap(),
        Some(Standing::Waiting(Waiting::Mechanical(Mechanical::Rename)))
    );
    // Another net change of the same branch is another branch.
    assert_eq!(standing(&db, branch(TWO)).unwrap(), None);
}

#[test]
fn the_answer_to_the_question_decides_whether_the_branch_is_reviewed() {
    let db = db();
    let answered = record_asked(&db, branch(ONE), 2500, &stamp()).unwrap();
    let dropped = record_asked(&db, branch(TWO), 2500, &stamp()).unwrap();
    assert_eq!(
        standing(&db, branch(ONE)).unwrap(),
        Some(Standing::Waiting(Waiting::Asked(answered)))
    );
    let decision = decision_answering(&db, answered);
    lists_record::close(&db, answered, How::Answered(decision), &stamp()).unwrap();
    lists_record::close(&db, dropped, How::Dropped("no".into()), &stamp()).unwrap();
    assert_eq!(
        standing(&db, branch(ONE)).unwrap(),
        Some(Standing::Approved(answered))
    );
    assert_eq!(
        standing(&db, branch(TWO)).unwrap(),
        Some(Standing::Waiting(Waiting::Declined(dropped)))
    );
}

#[test]
fn a_branch_is_recorded_once() {
    let db = db();
    record_mechanical(&db, branch(ONE), Mechanical::Design, &stamp()).unwrap();
    assert!(record_mechanical(&db, branch(ONE), Mechanical::Design, &stamp()).is_err());
}

#[test]
fn the_patch_id_ignores_where_the_lines_sit() {
    let diff = |at: usize| {
        format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -{at},1 +{at},2 @@\n keep\n+added\n")
    };
    assert_eq!(patch_id(&diff(3)).unwrap(), patch_id(&diff(40)).unwrap());
    assert_ne!(
        patch_id(&diff(3)).unwrap(),
        patch_id(&diff(3).replace("added", "other")).unwrap()
    );
}
