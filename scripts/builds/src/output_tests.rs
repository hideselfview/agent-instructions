use super::*;

fn site(line: u32) -> Site {
    ("a.rs", line, 1)
}

#[test]
fn a_kind_prints_its_first_lines_and_one_total_for_the_rest() {
    let mut ledger = Ledger::new();
    let printed: Vec<String> = (0..1000)
        .filter_map(|n| ledger.admit(site(1), Stream::Err, &format!("builds: deleted {n}")))
        .collect();
    assert_eq!(printed.len(), LINES_PER_KIND);
    assert_eq!(printed[0], "builds: deleted 0");
    assert_eq!(
        ledger.take_totals(),
        [(
            Stream::Err,
            "builds: left out 970 more lines of one kind, the last of them: builds: deleted 999"
                .to_string()
        )]
    );
    assert!(ledger.take_totals().is_empty(), "a total printed twice");
}

#[test]
fn each_kind_has_its_own_cap_and_total() {
    let mut ledger = Ledger::new();
    for n in 0..=LINES_PER_KIND {
        ledger.admit(site(1), Stream::Err, &format!("a {n}"));
    }
    assert_eq!(
        ledger.admit(site(2), Stream::Out, "b"),
        Some("b".to_string())
    );
    assert_eq!(
        ledger.take_totals(),
        [(
            Stream::Err,
            format!(
                "builds: left out 1 more line of one kind, the last of them: a {LINES_PER_KIND}"
            )
        )]
    );
}

#[test]
fn the_lines_of_one_message_count_one_by_one() {
    let mut ledger = Ledger::new();
    let long: Vec<String> = (0..100).map(|n| format!("line {n}")).collect();
    let printed = ledger
        .admit(site(1), Stream::Err, &long.join("\n"))
        .unwrap();
    assert_eq!(printed, long[..LINES_PER_KIND].join("\n"));
    assert_eq!(ledger.admit(site(1), Stream::Err, "more"), None);
    assert_eq!(
        ledger.take_totals(),
        [(
            Stream::Err,
            "builds: left out 71 more lines of one kind, the last of them: more".to_string()
        )]
    );
}

#[test]
fn a_service_leaves_nothing_out() {
    let mut ledger = Ledger::new();
    ledger.keep_every_line = true;
    for n in 0..1000 {
        assert!(ledger.admit(site(1), Stream::Err, &n.to_string()).is_some());
    }
    assert!(ledger.take_totals().is_empty());
}
