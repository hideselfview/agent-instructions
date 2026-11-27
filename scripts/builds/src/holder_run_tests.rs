use super::*;
use crate::testing::Scratch;

const JOINED: i64 = 1_790_000_000;
const MINUTE: i64 = 60;

#[test]
fn a_run_that_waited_then_ran_two_steps_shows_its_wait_its_step_and_its_building_time() {
    // The build joined the queue, waited 25 minutes for a checkout, ran
    // `macos` for 3 minutes, and started `macos-tests`, which the record
    // names in place of `macos`.
    let mut run = HolderRun {
        pid: 41,
        joined_at: JOINED,
        waited_ms: 25 * 60 * 1000,
        step: Some(RunningStep {
            name: "macos".into(),
            started_at: JOINED + 25 * MINUTE,
            waits: None,
        }),
    };
    assert_eq!(
        run.times(JOINED + 27 * MINUTE),
        RunTimes {
            moved_at: JOINED + 25 * MINUTE,
            waited_minutes: 25,
            building_minutes: 2,
            step: Some(StepTime {
                name: "macos".into(),
                minutes: 2,
                waits: None,
            }),
        }
    );
    run.step = Some(RunningStep {
        name: "macos-tests".into(),
        started_at: JOINED + 28 * MINUTE,
        waits: None,
    });
    assert_eq!(
        run.times(JOINED + 32 * MINUTE + 30),
        RunTimes {
            moved_at: JOINED + 28 * MINUTE,
            waited_minutes: 25,
            building_minutes: 7,
            step: Some(StepTime {
                name: "macos-tests".into(),
                minutes: 4,
                waits: None,
            }),
        }
    );
}

#[test]
fn a_run_before_its_first_step_shows_its_wait_and_no_step() {
    let run = HolderRun {
        pid: 41,
        joined_at: JOINED,
        waited_ms: 90 * 1000,
        step: None,
    };
    assert_eq!(
        run.times(JOINED + 4 * MINUTE),
        RunTimes {
            moved_at: JOINED + 90,
            waited_minutes: 1,
            building_minutes: 2,
            step: None,
        }
    );
}

#[test]
fn a_record_reads_back_as_written_and_a_missing_one_is_none() {
    let scratch = Scratch::new("holder-run-file");
    let file = scratch.dir.join("side-1.run");
    assert_eq!(HolderRun::read(&file).unwrap(), None);
    let run = HolderRun {
        pid: 41,
        joined_at: JOINED,
        waited_ms: 1500,
        step: Some(RunningStep {
            name: "macos".into(),
            started_at: JOINED + MINUTE,
            waits: Some(crate::host_display::Wait {
                host: "m1".into(),
                target: Some("u@m1".into()),
                reasons: vec![crate::host_display::Reason::LidClosed],
            }),
        }),
    };
    run.write(&file).unwrap();
    assert_eq!(HolderRun::read(&file).unwrap(), Some(run));
    std::fs::write(&file, "{").unwrap();
    assert!(HolderRun::read(&file).is_err(), "a damaged record fails");
}
