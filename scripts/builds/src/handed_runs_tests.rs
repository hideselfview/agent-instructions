use super::*;
use crate::app_timing::{BuiltCommit, DATABASE, RunRecord, StepKind, StepRecord, StepResult};
use crate::install_gate::{self, PerConnection};
use crate::lane::Lane;
use crate::run_end::RunEnd;
use crate::testing::Scratch;

fn run() -> RunRecord {
    RunRecord {
        started_at: 1_000_000,
        repo: "bae".to_string(),
        lane: Lane::MAC,
        checkout: "side-1".to_string(),
        built_ref: "branch".to_string(),
        requested_commit: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
        built_commit: Some(BuiltCommit {
            sha: "0123456789abcdef0123456789abcdef01234567".into(),
            reused_shots: vec![shots::Prefix::parse("mac.a.").unwrap()],
        }),
        who: Some("agent".to_string()),
        wait_ms: 2_000,
        outcome: RunEnd::Passed,
        load: None,
        steps: vec![StepRecord {
            at_merge_base: None,
            name: "macos".to_string(),
            kind: StepKind::Step,
            started_at: 1_000_002,
            wall_ms: 60_000,
            cpu_ms: 30_000,
            result: StepResult::Passed,
            shots: None,
        }],
    }
}

#[test]
fn a_passing_run_reaches_the_installed_builds_after_its_project_schema_moves() {
    let scratch = Scratch::new("handed-runs-project-move");
    let places = scratch.places();
    let project = crate::lists::fixtures::make_store(&places, &["bae"]);
    let database = project.dir.join(DATABASE);
    install_gate::use_per_connection(
        PerConnection::new(places.clone(), 23, "app-build bae branch", "10:00:00").unwrap(),
    );

    let quiet = &mut |line| panic!("install waited for the app build: {line}");
    let closed = install_gate::begin(&places, quiet)
        .unwrap()
        .close(quiet)
        .unwrap();
    let mut conn = rusqlite::Connection::open(&database).unwrap();
    let migration = conn.transaction().unwrap();
    migration
        .execute_batch("CREATE TABLE new_builds_data (value TEXT NOT NULL);")
        .unwrap();
    migration
        .pragma_update(None, "user_version", crate::db::stamp() + 1)
        .unwrap();
    migration.commit().unwrap();
    drop(conn);
    drop(closed);

    let completed = run();
    let mut handed = None;
    write(
        &places,
        &completed,
        None,
        install_gate::install_ran,
        |store| {
            handed = Some((
                store.to_path_buf(),
                crate::app_timing::read_named_run(store)?,
            ));
            Ok(())
        },
    )
    .unwrap();
    let (store, saved) = handed.expect("the lane hands its run to the installed builds");
    assert_eq!(saved, completed);
    assert!(store.starts_with(folder(&places)), "{}", store.display());
    assert!(!store.exists(), "the lane deletes the store it handed");
}

/// A store of another schema with no install since the lane started is
/// another fault: the record fails, and the lane hands nothing.
#[test]
fn a_store_of_another_schema_with_no_install_fails_the_record() {
    let scratch = Scratch::new("handed-runs-no-install");
    let places = scratch.places();
    let project = crate::lists::fixtures::make_store(&places, &["bae"]);
    let conn = rusqlite::Connection::open(project.dir.join(DATABASE)).unwrap();
    conn.pragma_update(None, "user_version", 7).unwrap();
    drop(conn);
    let Err(error) = write(
        &places,
        &run(),
        None,
        || Ok(false),
        |_| panic!("no install ran, so the lane hands nothing"),
    ) else {
        panic!("expected the record to fail");
    };
    let said = error.text();
    assert!(said.contains("another builds schema"), "{said}");
}

/// The lane writes to the project's store directly when no install moved it.
#[test]
fn the_lane_records_its_run_directly_if_no_install_moved_the_stores() {
    let scratch = Scratch::new("handed-runs-same");
    let places = scratch.places();
    let project = crate::lists::fixtures::make_store(&places, &["bae"]);
    write(
        &places,
        &run(),
        None,
        || Ok(false),
        |_| panic!("no install moved the stores, so the lane hands nothing"),
    )
    .unwrap();
    assert_eq!(
        crate::app_timing::read_named_run(&project.dir.join(DATABASE)).unwrap(),
        run()
    );
}

/// When the installed builds cannot record the handed run, the record fails
/// with what it said, and the lane still deletes the store it handed.
#[test]
fn a_hand_off_that_fails_fails_the_record_and_leaves_no_store() {
    let scratch = Scratch::new("handed-runs-fail");
    let places = scratch.places();
    let mut store = None;
    let Err(error) = write(
        &places,
        &run(),
        None,
        || Ok(true),
        |handed| {
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

/// The installed builds moves a handed store to its schema and records the
/// run it holds in the store of the repo's project.
#[test]
fn the_installed_builds_records_the_run_of_a_handed_store() {
    let scratch = Scratch::new("handed-runs-receive");
    let places = scratch.places();
    let project = crate::lists::fixtures::make_store(&places, &["bae"]);
    let handed = scratch.dir.join("handed").join(DATABASE);
    std::fs::create_dir_all(handed.parent().unwrap()).unwrap();
    drop(crate::db::open(&handed, crate::db::Make::Allowed).unwrap());
    crate::app_timing::record(&handed, &run()).unwrap();
    receive(&places, "bae", &handed, None, 1_000_100).unwrap();
    assert_eq!(
        crate::app_timing::read_named_run(&project.dir.join(DATABASE)).unwrap(),
        run()
    );
}

#[test]
fn the_installed_builds_refuses_a_handoff_that_names_another_repo() {
    let scratch = Scratch::new("handed-runs-wrong-repo");
    let places = scratch.places();
    let project = crate::lists::fixtures::make_store(&places, &["other"]);
    let handed = scratch.dir.join(DATABASE);
    drop(crate::db::open(&handed, crate::db::Make::Allowed).unwrap());
    crate::app_timing::record(&handed, &run()).unwrap();

    let said = receive(&places, "other", &handed, None, 1_000_100)
        .unwrap_err()
        .text();
    assert!(
        said.contains("repo bae") && said.contains("other"),
        "{said}"
    );
    assert!(
        crate::app_timing::branch_ends(&rusqlite::Connection::open(project.database()).unwrap())
            .unwrap()
            .is_empty()
    );
}

/// A lane of a repo two projects hold records its run in the store of the
/// project it was told, whether it writes the run itself or hands it to the
/// installed builds, and a run with no project named is refused.
#[test]
fn a_two_project_repos_run_goes_to_the_store_of_its_project() {
    let scratch = Scratch::new("handed-runs-two-projects");
    let places = scratch.places();
    let first = crate::lists::fixtures::make_store_named(&places, "first", &["bae"]);
    let second = crate::lists::fixtures::make_store_named(&places, "second", &["bae"]);
    let runs = |store: &crate::lists::store::Store| {
        let db = rusqlite::Connection::open(store.database()).unwrap();
        crate::app_timing::branch_ends(&db).unwrap().len()
    };
    let no_hand_off = |_: &Path| panic!("no install moved the stores");
    write(&places, &run(), Some("second"), || Ok(false), no_hand_off).unwrap();
    assert_eq!((runs(&first), runs(&second)), (0, 1));
    let handed = scratch.dir.join(DATABASE);
    drop(crate::db::open(&handed, crate::db::Make::Allowed).unwrap());
    crate::app_timing::record(&handed, &run()).unwrap();
    let refused = receive(&places, "bae", &handed, None, 1_000_100)
        .unwrap_err()
        .text();
    assert!(refused.contains("two projects"), "{refused}");
    receive(&places, "bae", &handed, Some("first"), 1_000_100).unwrap();
    assert_eq!((runs(&first), runs(&second)), (1, 1));
}
