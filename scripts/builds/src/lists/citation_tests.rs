use super::*;

#[test]
fn decision_citations_name_decision_and_a_number() {
    assert_eq!(
        decision_ids("Decision #12 and decision#3, not decisions #4, and decision #12 again"),
        vec![Id(12), Id(3)]
    );
    assert_eq!(decision_ids("the decision is yours"), Vec::<Id>::new());
}
