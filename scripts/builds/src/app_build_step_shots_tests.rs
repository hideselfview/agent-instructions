use super::*;
use crate::app_timing::{DATABASE, RunRecord, StepKind, StepResult};
use crate::lane::Lane;
use crate::run_end::RunEnd;
use crate::shots_cache::MissCause;
use crate::testing::Scratch;

/// `read_run` for a run of an app named for its platform.
fn read_named_run(path: &std::path::Path) -> crate::fail::Result<crate::app_timing::RunRecord> {
    crate::app_timing::read_run(path, |_, word| {
        Ok(crate::lane::Lane::named_for(word).expect("a platform's app"))
    })
}

const TOOK: &str = "builds shots: took the 12 mac shots of app tree 0123abcd from the picture folder; capturing none";

#[test]
fn the_checkers_line_for_shots_taken_from_the_picture_folder_gives_their_count() {
    assert_eq!(reused_in(TOOK), Some(Ok(12)));
    assert_eq!(reused_in(&format!("[mac side-1] {TOOK}")), Some(Ok(12)));
    assert_eq!(
        reused_in("builds shots: comparing with the run of abc"),
        None
    );
    assert!(
        matches!(
            reused_in(
                "builds shots: took the many mac shots from the picture folder; capturing none"
            ),
            Some(Err(_))
        ),
        "a count that is no number is an error"
    );
}

#[test]
fn a_read_adds_up_the_lines_that_say_shots_came_from_the_picture_folder() {
    let read = ReusedRead::default();
    let each = read.each_line();
    each("Compiling shots");
    each(TOOK);
    each(
        "builds shots: took the 3 android shots of app tree x from the picture folder; capturing none",
    );
    assert_eq!(read.reused(), Ok(15));
    assert_eq!(ReusedRead::default().reused(), Ok(0));
    each("builds shots: took the many mac shots from the picture folder; capturing none");
    each(TOOK);
    assert!(read.reused().is_err(), "a bad count stays an error");
}

#[test]
fn a_finished_step_took_every_shot_from_the_folder_or_shot_every_one() {
    assert_eq!(
        Pictures::of(Some(12), 12),
        Some(Pictures {
            reused: 12,
            captured: 0
        })
    );
    assert_eq!(
        Pictures::of(Some(12), 0),
        Some(Pictures {
            reused: 0,
            captured: 12
        })
    );
    assert_eq!(Pictures::of(None, 0), None);
}

fn step(name: &str, kind: StepKind, shots: Option<ShotsUse>) -> StepRecord {
    StepRecord {
        name: name.to_string(),
        kind,
        started_at: 1_000_002,
        wall_ms: 1,
        cpu_ms: 1,
        at_merge_base: None,
        result: StepResult::Passed,
        shots,
    }
}

fn used(base_run: BaseRun, pictures: Option<Pictures>) -> Option<ShotsUse> {
    Some(ShotsUse {
        environment: "26B131-17C529".to_string(),
        base_run,
        pictures,
    })
}

/// Steps of each kind: a step with no shots, a shots step whose base run was
/// saved and whose shots came from the folder, one whose base run was built
/// and that shot its screens, one that compared with nothing and ended before
/// it counted its screens.
fn steps() -> Vec<StepRecord> {
    vec![
        step("macos", StepKind::Step, None),
        step(
            "shots mac.a.",
            StepKind::Shots,
            used(
                BaseRun::Saved,
                Some(Pictures {
                    reused: 12,
                    captured: 0,
                }),
            ),
        ),
        step(
            "shots mac.b.",
            StepKind::Shots,
            used(
                BaseRun::Built(MissCause::BoardChanged),
                Some(Pictures {
                    reused: 0,
                    captured: 7,
                }),
            ),
        ),
        step(
            "shots mac.c.",
            StepKind::Shots,
            used(BaseRun::NotNeeded, None),
        ),
    ]
}

#[test]
fn each_shots_step_keeps_its_base_run_and_its_screenshots_in_the_store() {
    let scratch = Scratch::new("step-shots-record");
    let store = crate::lists::fixtures::make_store_named(&scratch.places(), "bae", &["bae"]);
    let run = RunRecord {
        started_at: 1_000_000,
        repo: "bae".to_string(),
        lane: Lane::MAC,
        checkout: "side-1".to_string(),
        built_ref: "branch".to_string(),
        requested_commit: None,
        built_commit: None,
        who: None,
        wait_ms: 0,
        outcome: RunEnd::Passed,
        load: None,
        steps: steps(),
    };
    crate::app_timing::record(&store.database(), &run).unwrap();
    let conn = crate::db::open(&store.database(), crate::db::Make::Never).unwrap();
    let rows: Vec<String> = conn
        .prepare("SELECT position || ' ' || environment || ' ' || base_run || ' ' || ifnull(reused, '-') || ' ' || ifnull(captured, '-') FROM app_build_step_shots ORDER BY position")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [
            "1 26B131-17C529 saved 12 0",
            "2 26B131-17C529 built 0 7",
            "3 26B131-17C529 not needed - -"
        ]
    );
}

#[test]
fn a_run_read_back_holds_the_steps_with_what_they_used() {
    let scratch = Scratch::new("step-shots-read");
    let path = scratch.dir.join("one").join(DATABASE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    drop(crate::db::open(&path, crate::db::Make::Allowed).unwrap());
    let run = RunRecord {
        started_at: 1_000_000,
        repo: "bae".to_string(),
        lane: Lane::MAC,
        checkout: "side-1".to_string(),
        built_ref: "branch".to_string(),
        requested_commit: None,
        built_commit: None,
        who: None,
        wait_ms: 0,
        outcome: RunEnd::Passed,
        load: None,
        steps: steps(),
    };
    crate::app_timing::record(&path, &run).unwrap();
    assert_eq!(read_named_run(&path).unwrap(), run);
}

#[test]
fn a_base_run_word_the_store_does_not_know_is_refused() {
    assert!(BaseRun::from_word("saved", None).is_ok());
    assert!(BaseRun::from_word("missing", None).is_err());
    assert!(
        BaseRun::from_word("built", None).is_err(),
        "a built step stores its cause"
    );
}
