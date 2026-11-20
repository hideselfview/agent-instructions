use super::*;

#[test]
fn a_failed_test_comes_from_cargo_tests_and_nextests_lines() {
    assert_eq!(
        failed_test("test card_profile::tests::x ... FAILED"),
        Some("card_profile::tests::x")
    );
    assert_eq!(
        failed_test("        FAIL [   0.004s] (1/5) builds::bin/builds card_profile::tests::x"),
        Some("card_profile::tests::x")
    );
    assert_eq!(
        failed_test("     TIMEOUT [ 120.000s] bae-store store::tests::slow"),
        Some("store::tests::slow")
    );
    assert_eq!(failed_test("test card_profile::tests::x ... ok"), None);
    assert_eq!(
        failed_test("        PASS [   0.004s] builds card_profile::tests::x"),
        None
    );
}

#[test]
fn a_failed_run_keeps_its_failed_tests_once_and_the_lines_from_the_first_panic() {
    let watch = Watch::default();
    let mut out = Tee::new(Vec::new(), watch.clone());
    for line in [
        "    Compiling builds v0.1.0\n",
        "        FAIL [   0.004s] builds a::one\n",
        "thread 'a::one' panicked at src/a.rs:3:5:\n",
        "assertion `left == right` failed\n",
        "        FAIL [   0.004s] builds a::one\n",
    ] {
        out.write_all(line.as_bytes()).unwrap();
    }
    drop(out);
    let outcome = watch.outcome(100);
    assert_eq!(outcome.ended, RunEnd::Failed);
    assert_eq!(outcome.failed, vec!["a::one".to_string()]);
    assert_eq!(
        outcome.excerpt.as_deref(),
        Some(
            "thread 'a::one' panicked at src/a.rs:3:5:\nassertion `left == right` failed\n        FAIL [   0.004s] builds a::one"
        )
    );
}

#[test]
fn a_passed_run_keeps_no_excerpt_and_a_signal_stops_a_run() {
    let watch = Watch::default();
    watch.line(b"error: something\n");
    assert_eq!(watch.outcome(0).excerpt, None);
    assert_eq!(watch.outcome(0).ended, RunEnd::Passed);
    assert_eq!(watch.outcome(130).ended, RunEnd::Stopped);
    assert_eq!(
        Watch::default().outcome(101),
        Outcome {
            ended: RunEnd::Failed,
            failed: Vec::new(),
            excerpt: None,
        },
        "builds read no output"
    );
}

/// A writer whose every write fails.
struct Closed;

impl Write for Closed {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("closed"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_watch_reads_every_line_even_after_printing_failed() {
    let watch = Watch::default();
    let mut tee = Tee::new(Closed, watch.clone());
    assert!(
        tee.write_all_watched(b"test a::one ... FAILED\n", true)
            .is_err()
    );
    tee.write_all_watched(b"test a::two ... FAILED\n", false)
        .unwrap();
    drop(tee);
    assert_eq!(watch.outcome(101).failed, vec!["a::one", "a::two"]);
}
