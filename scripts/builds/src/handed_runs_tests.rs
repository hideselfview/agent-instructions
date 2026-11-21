use super::*;
use crate::app_timing::{DATABASE, RunRecord, StepKind, StepRecord, StepResult, read_runs};
use crate::lane::Lane;
use crate::run_end::RunEnd;
use crate::testing::Scratch;

fn run() -> RunRecord {
    RunRecord {
        started_at: 1_000_000,
        repo: "bae".to_string(),
        lane: Lane::Mac,
        checkout: "side-1".to_string(),
        built_ref: "branch".to_string(),
        built_commit: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
        who: Some("agent".to_string()),
        wait_ms: 2_000,
        outcome: RunEnd::Passed,
        load: None,
        steps: vec![StepRecord {
            name: "macos".to_string(),
            kind: StepKind::Step,
            started_at: 1_000_002,
            wall_ms: 60_000,
            cpu_ms: 30_000,
            result: StepResult::Passed,
        }],
    }
}

/// A store file whose schema is not this program's, as a store is once an
/// install moved it to the installed program's schema.
fn moved_store(scratch: &Scratch) -> std::path::PathBuf {
    let path = scratch.dir.join("moved").join(DATABASE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 7).unwrap();
    path
}

/// When an install moved the store while the lane built, the lane writes its
/// run to a store of its own schema, hands that store to the installed
/// builds, and deletes it once the installed builds has recorded it.
#[test]
fn a_run_whose_store_an_install_moved_goes_to_the_installed_builds() {
    let scratch = Scratch::new("handed-runs-hand");
    let moved = moved_store(&scratch);
    let folder = scratch.dir.join("handed");
    let mut handed = Vec::new();
    write(
        &moved,
        &run(),
        &folder,
        &|| Ok(true),
        &mut |store: &Path| {
            handed.push((store.to_path_buf(), read_runs(store).unwrap()));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(handed.len(), 1, "the lane hands one store");
    let (store, runs) = &handed[0];
    assert_eq!(runs, &vec![run()], "the handed store holds the run");
    assert!(store.starts_with(&folder), "{}", store.display());
    assert!(!store.exists(), "the lane deletes the store it handed");
}

/// A store of another schema with no install since the lane started is
/// another fault: the record fails, and the lane hands nothing.
#[test]
fn a_store_of_another_schema_with_no_install_fails_the_record() {
    let scratch = Scratch::new("handed-runs-no-install");
    let moved = moved_store(&scratch);
    let folder = scratch.dir.join("handed");
    let Err(error) = write(&moved, &run(), &folder, &|| Ok(false), &mut |_: &Path| {
        panic!("no install ran, so the lane hands nothing")
    }) else {
        panic!("expected the record to fail");
    };
    let said = error.text();
    assert!(said.contains("another builds schema"), "{said}");
}

/// A store of this program's schema takes the run, whether or not an install
/// ran, and the lane hands nothing.
#[test]
fn a_store_of_this_schema_takes_the_run_after_an_install() {
    let scratch = Scratch::new("handed-runs-same");
    let store = scratch.dir.join(DATABASE);
    drop(crate::db::open(&store, crate::db::Make::Allowed).unwrap());
    let folder = scratch.dir.join("handed");
    write(&store, &run(), &folder, &|| Ok(true), &mut |_: &Path| {
        panic!("the store has this schema, so the lane hands nothing")
    })
    .unwrap();
    assert_eq!(read_runs(&store).unwrap(), vec![run()]);
}

/// When the installed builds cannot record the handed run, the record fails
/// with what it said, and the lane still deletes the store it handed.
#[test]
fn a_hand_off_that_fails_fails_the_record_and_leaves_no_store() {
    let scratch = Scratch::new("handed-runs-fail");
    let moved = moved_store(&scratch);
    let folder = scratch.dir.join("handed");
    let mut store = None;
    let Err(error) = write(
        &moved,
        &run(),
        &folder,
        &|| Ok(true),
        &mut |handed: &Path| {
            store = Some(handed.to_path_buf());
            crate::fail!("the installed builds refused it")
        },
    ) else {
        panic!("expected the record to fail");
    };
    let said = error.text();
    assert!(said.contains("refused it"), "{said}");
    assert!(!store.unwrap().exists());
}

/// The installed builds moves a handed store to its schema and records each
/// run it holds in the store of the repo's project.
#[test]
fn the_installed_builds_records_the_runs_of_a_handed_store() {
    let scratch = Scratch::new("handed-runs-receive");
    let places = scratch.places();
    let project = crate::lists::fixtures::make_store(&places, &["bae"]);
    let handed = scratch.dir.join("handed").join(DATABASE);
    std::fs::create_dir_all(handed.parent().unwrap()).unwrap();
    drop(crate::db::open(&handed, crate::db::Make::Allowed).unwrap());
    crate::app_timing::record(&handed, &run()).unwrap();
    receive(&places, "bae", &handed, 1_000_100).unwrap();
    assert_eq!(read_runs(&project.dir.join(DATABASE)).unwrap(), vec![run()]);
}
