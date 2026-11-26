use super::*;
use crate::health::{Advice, FindingKind};

fn finding(text: &str) -> Finding {
    Finding::new(FindingKind::Still, Advice::said(text.to_string()))
}

/// Two runs of the same wait differ in times, process ids, loads and CPU
/// shares, and keep one key.
#[test]
fn a_line_keeps_its_key_when_only_times_pids_loads_and_shares_change() {
    let first = "item #2311 \"Compose\" of project bae last moved 1h23m ago and waits on process 6154 on this Mac (/bin/zsh -c source /x/snapshot-zsh-179-x3ay.sh); its 5-minute load is 10.6 on 14 cores; CPU 29% idle; swap 14M used";
    let second = "item #2311 \"Compose\" of project bae last moved 6d4h ago and waits on process 77 on this Mac (/bin/zsh -c source /x/snapshot-zsh-180-q1.sh); its 5-minute load is 12.0 on 14 cores; CPU 3% idle; swap 906M used";
    assert_eq!(key(first), key(second));
}

#[test]
fn a_line_about_another_record_has_another_key() {
    assert_ne!(
        key("item #2311 last moved 1h ago"),
        key("item #2312 last moved 1h ago")
    );
    assert_ne!(
        key("review 21 of bae must fix confirmed findings 368, 369"),
        key("review 21 of bae must fix confirmed findings 368, 370")
    );
}

#[test]
fn a_run_marks_the_lines_the_earlier_run_lacked() {
    let findings = [
        finding("item #1 last moved 5m ago"),
        finding("item #2 last moved 9m ago"),
    ];
    let mut out = vec![
        "needs action:".to_string(),
        "    item #1 last moved 5m ago".to_string(),
        "    item #2 last moved 9m ago".to_string(),
    ];
    let earlier = vec![key("item #1 last moved 1h2m ago")];
    mark(&mut out, &findings, &earlier);
    assert_eq!(out[1], "    item #1 last moved 5m ago");
    assert_eq!(out[2], "    new: item #2 last moved 9m ago");
}

/// A caller with no earlier run has every line marked new.
#[test]
fn a_first_run_marks_every_line() {
    let findings = [finding("item #1 last moved 5m ago")];
    let mut out = vec![
        "needs action:".to_string(),
        "    item #1 last moved 5m ago".to_string(),
    ];
    mark(&mut out, &findings, &[]);
    assert_eq!(out[1], "    new: item #1 last moved 5m ago");
}

/// A caller's record holds what its last run printed; another caller has none,
/// and a record that is not a list of keys fails to read.
#[test]
fn each_caller_keeps_its_own_record() {
    let scratch = crate::testing::Scratch::new("health-new");
    assert!(read(&scratch.dir, "a1").unwrap().is_empty());
    write(&scratch.dir, "a1", &[finding("item #1 last moved 5m ago")]).unwrap();
    assert_eq!(
        read(&scratch.dir, "a1").unwrap(),
        vec![key("item #1 last moved 9m ago")]
    );
    assert!(read(&scratch.dir, "a2").unwrap().is_empty());
    std::fs::write(file(&scratch.dir, "a1"), "not json").unwrap();
    assert!(read(&scratch.dir, "a1").is_err());
}
