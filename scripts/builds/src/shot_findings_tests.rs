use super::*;

#[test]
fn the_findings_are_the_text_after_cargos_running_line_or_all_of_it_without_one() {
    let output = "   Compiling a\n    Finished dev\n     Running `target/debug/bae-shots run`\nFAIL x: y\nbae-shots: done\n";
    assert_eq!(findings_of(output), "FAIL x: y\nbae-shots: done\n");
    assert_eq!(findings_of("FAIL x: y\n"), "FAIL x: y\n");
}

#[test]
fn a_run_has_findings_when_it_passed_or_said_that_pairs_fail_it() {
    assert!(has_findings(0, ""));
    assert!(has_findings(1, "bae-shots: 2 pair(s) fail the run: a, b\n"));
    assert!(!has_findings(1, "bae-shots: the simulator did not start\n"));
}

#[test]
fn only_the_last_megabyte_of_a_long_run_is_kept_cut_at_a_character() {
    let long = "é".repeat(KEEP);
    let kept = findings_of(&long);
    assert!(kept.len() <= KEEP && kept.len() > KEEP - 4);
    assert!(kept.chars().all(|c| c == 'é'));
}

#[test]
fn a_stored_run_is_found_by_its_whole_key_and_a_later_run_of_the_key_replaces_it() {
    let scratch = crate::testing::Scratch::new("shots-key");
    let places = scratch.places();
    let store = crate::lists::fixtures::make_store(&places, &["demo"]);
    let database = store.dir.join(app_timing::DATABASE);
    let key = Key {
        repo: "demo".to_string(),
        commit: "a".repeat(40),
        lane: Lane::Mac,
        prefix: "mac.a.".to_string(),
        version: "b".repeat(40),
    };
    assert!(lookup(&database, &key).unwrap().is_none());
    store_run(&database, &key, 1, "first\n");
    store_run(&database, &key, 0, "second\n");
    let found = lookup(&database, &key).unwrap().expect("stored");
    assert_eq!((found.exit_code, found.findings.as_str()), (0, "second\n"));
    for other in [
        Key {
            commit: "c".repeat(40),
            ..key.clone()
        },
        Key {
            lane: Lane::Android,
            ..key.clone()
        },
        Key {
            prefix: "mac.b.".to_string(),
            ..key.clone()
        },
        Key {
            version: "c".repeat(40),
            ..key.clone()
        },
        Key {
            repo: "other".to_string(),
            ..key.clone()
        },
    ] {
        assert!(lookup(&database, &other).unwrap().is_none(), "{other:?}");
    }
}

fn store_run(database: &Path, key: &Key, exit: i64, findings: &str) {
    store(database, key, exit, findings).unwrap();
}
