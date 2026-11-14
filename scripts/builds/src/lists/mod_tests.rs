use super::*;

#[test]
fn a_side_worktree_path_names_its_repo_and_checkout() {
    assert_eq!(
        side_worktree_parts("/Users/x/dev/bae-worktrees/side-12"),
        Some(("bae".to_string(), "side-12".to_string()))
    );
    for bad in [
        "bae-worktrees/side-1",
        "/dev/bae/side-1",
        "/dev/bae-worktrees/side-",
        "/dev/bae-worktrees/side-1a",
        "/dev/bae-worktrees/main",
        "/dev/-worktrees/side-1",
    ] {
        assert_eq!(side_worktree_parts(bad), None, "{bad}");
    }
}

#[test]
fn wire_shapes_are_the_plans() {
    let wait: Vec<Wait> =
        serde_json::from_str(r#"[{"item":4},{"question":3},{"design":9},{"condition":"quiet"}]"#)
            .unwrap();
    assert_eq!(
        wait,
        vec![
            Wait::Item(Id(4)),
            Wait::Question(Id(3)),
            Wait::Design(Id(9)),
            Wait::Condition("quiet".into())
        ]
    );
    assert_eq!(
        serde_json::to_string(&How::MovedToStack(Id(2))).unwrap(),
        r#"{"moved-to-stack":2}"#
    );
    assert_eq!(serde_json::to_string(&How::Done).unwrap(), r#""done""#);
    assert_eq!(
        serde_json::to_string(&Audience::DesignSession).unwrap(),
        r#""design-session""#
    );
    // Unknown fields are refused.
    assert!(serde_json::from_str::<Stamp>(r#"{"at":1,"by":"x","extra":1}"#).is_err());
}
