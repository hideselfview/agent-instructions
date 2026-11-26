use super::*;
use crate::app_landing::covers_prefix;
use crate::app_timing::PassedRun;
use crate::lists::fixtures::{db, design, design_place, item, read};

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn names(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

const BOARD: &str = "design/mocks/mac.identify.found.dc.html";

#[test]
fn a_board_change_that_waits_on_an_open_item_leaves_its_scene_out() {
    let db = db();
    let work = item(&db, vec![]);
    let change = design(&db);
    design_place(&db, change, BOARD, Resolution::Item(work));
    let lists = read(&db);
    assert_eq!(
        waiting_in(&lists, "bae", &strings(&[BOARD])),
        [Waiting {
            scene: "mac.identify.found".into(),
            items: vec![work],
        }]
    );
    // A branch that leaves the board as it is has nothing to wait for.
    assert_eq!(waiting_in(&lists, "bae", &strings(&["src/a.rs"])), []);
    // Another repo's boards are none of this repo's.
    assert_eq!(waiting_in(&lists, "coven", &strings(&[BOARD])), []);
}

#[test]
fn only_a_board_place_with_open_app_work_waits() {
    let db = db();
    let work = item(&db, vec![]);
    let change = design(&db);
    design_place(
        &db,
        change,
        BOARD,
        Resolution::Matches("the app shows it".into()),
    );
    design_place(
        &db,
        change,
        "design/system/components/MacRow/",
        Resolution::Item(work),
    );
    let lists = read(&db);
    let changed = strings(&[BOARD, "design/system/components/MacRow/a.html"]);
    assert_eq!(waiting_in(&lists, "bae", &changed), []);
}

#[test]
fn a_prefix_runs_the_names_it_selects_less_the_waiting_scenes() {
    let waiting = [Waiting {
        scene: "mac.identify.found".into(),
        items: vec![Id(7)],
    }];
    let declared = names(&[
        "mac.identify.found",
        "mac.identify.searching",
        "mac.welcome.start",
    ]);
    let Split { kept, left_out } = split("mac.identify.", &declared, &waiting).unwrap();
    assert_eq!(kept, ["mac.identify.searching"]);
    assert_eq!(left_out, [&waiting[0]]);
    assert_eq!(split("mac.welcome.", &declared, &waiting), None);
}

fn run(ran: &[&str]) -> PassedRun {
    PassedRun {
        commit: "built".to_string().into(),
        steps: ran.iter().map(|p| format!("shots {p}")).collect(),
    }
}

fn covers(ran: &[&str], waiting: &[&str]) -> bool {
    let declared = names(&["mac.identify.found", "mac.identify.searching"]);
    covers_prefix(
        &run(ran),
        "mac.identify.",
        (Lane::Mac, "branch"),
        &mut |_, _| Some(declared.clone().into()),
        &names(waiting),
    )
}

#[test]
fn a_run_of_the_other_scenes_covers_a_prefix_whose_rest_waits() {
    assert!(!covers(&["mac.identify.searching"], &[]));
    assert!(covers(&["mac.identify.searching"], &["mac.identify.found"]));
}

#[test]
fn a_prefix_whose_scenes_all_wait_needs_no_run() {
    assert!(covers(
        &[],
        &["mac.identify.found", "mac.identify.searching"]
    ));
    assert!(!covers(&[], &["mac.identify.found"]));
}
