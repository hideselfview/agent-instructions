use super::*;
use crate::testing::Scratch;

/// A record of the session of this test process, written at <started_at> ms.
fn record(scratch: &Scratch, started_at: i64) {
    std::fs::write(
        scratch.dir.join(format!("{}.json", std::process::id())),
        format!(
            r#"{{"pid":{},"sessionId":"s1","startedAt":{started_at}}}"#,
            std::process::id()
        ),
    )
    .unwrap();
}

#[test]
fn a_session_whose_process_started_before_its_record_runs() {
    let scratch = Scratch::new("sessions-live");
    record(
        &scratch,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
    );
    assert_eq!(running(&scratch.dir).unwrap().len(), 1);
}

#[test]
fn a_pid_that_a_later_process_took_is_no_session() {
    let scratch = Scratch::new("sessions-taken");
    // The record is from long before this process started.
    record(&scratch, 1_000_000_000_000);
    assert!(running(&scratch.dir).unwrap().is_empty());
}
