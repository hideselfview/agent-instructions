use super::*;

#[test]
fn a_base_lane_holds_cached_pictures_before_and_between_comparisons() {
    let scratch = Scratch::new("base-lane-picture-lock");
    let places = scratch.places();
    let held = held_in(&scratch);
    let top = scratch.dir.join("repo");
    shots_repo(&top);
    let checker = scratch.dir.join("checker");
    let config_text = format!(
        "step mac macos false\nshots mac {} shots run --select {{select}}\nshots-declared mac app/shots\n",
        checker.display()
    );
    std::fs::write(top.join(CONFIG), &config_text).unwrap();
    std::fs::write(top.join(".gitignore"), "shots/\n").unwrap();
    std::fs::write(
        top.join("app/shots/Shots.swift"),
        "let scenes = [\"mac.a.one\", \"mac.b.one\"]\n",
    )
    .unwrap();
    crate::testing::git(&top, &["add", CONFIG, ".gitignore", "app"]);
    crate::testing::git(&top, &["commit", "-qm", "Declare two shots"]);
    let base = resolve(&top, "HEAD").unwrap();
    crate::testing::git(&top, &["checkout", "-qb", "change"]);
    crate::testing::git(&top, &["commit", "--allow-empty", "-qm", "Branch change"]);
    let head = resolve(&top, "HEAD").unwrap();
    let config = parse_config(&config_text).unwrap();
    let tree = picture_tree(&top, &base, &config, Lane::Mac, ENVIRONMENT).unwrap();
    let marks = shots::CaptureMarks::of(&shots_cache::app_picture_tree(
        &places,
        "demo",
        Lane::Mac,
        &tree,
    ));
    let prefixes = ["mac.a.", "mac.b."].map(|prefix| shots::Prefix::parse(prefix).unwrap());
    marks.write(&prefixes, false).unwrap();
    let taken = marks.directory();

    // The checker counts scenes before each comparison takes the root lock.
    // At that point a competing process must still be unable to lock the tree.
    scratch.script(
        "checker",
        &format!(
            r#"set -e
case "$2" in
    scenes)
        python3 - {anchor} <<'PY'
import fcntl, os, sys

marks = os.open(sys.argv[1], os.O_RDONLY)
try:
    fcntl.flock(marks, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    pass
else:
    raise SystemExit("the lane lost its picture lock")
finally:
    os.close(marks)
PY
        echo 1 ;;
    run)
        case "$*" in *--no-capture*--base) ;; *) exit 2 ;; esac
        {report} ;;
    *) exit 2 ;;
esac"#,
            anchor = crate::host::sh_quote(&taken.display().to_string()),
            report = report_run(&crate::shots_program::VERSIONS)
        ),
    );
    let programs = crate::testing::pinned_programs(&scratch.dir, &places.home);
    let picks = prefixes
        .clone()
        .map(|prefix| Pick::Shots(Lane::Mac, prefix));
    let needs = ["macos".to_string()];
    let build = LaneBuild {
        places: &places,
        repo: "demo",
        lane: Lane::Mac,
        request: LaneRequest {
            commit: &head,
            named: "change",
            names: &needs,
            picks: &picks,
            reused_shots: &[],
            needs_only: true,
            fresh: false,
        },
        programs: &programs,
        source: top.clone(),
        config: parse_config(&config_text).unwrap(),
        windowed: Vec::new(),
        base: None,
    };
    let merge_base = MergeBase {
        commit: base,
        config,
        names: needs.to_vec(),
        prefixes: prefixes.to_vec(),
        windowed: Vec::new(),
    };
    let mut site = Site::Local(top, scratch.dir.join("no.xcconfig"), held.checkout_lock());
    let mut timing = Timing::start();

    let (result, ran) = build
        .ensure_base(
            &merge_base,
            ENVIRONMENT,
            &mut site,
            &held,
            &scratch.dir.join("built"),
            &mut timing,
        )
        .unwrap();
    assert_eq!(result, Ended::Finished);
    assert_eq!(
        ran,
        prefixes
            .iter()
            .map(|prefix| (prefix.clone(), crate::shots_cache::MissCause::NoneSaved))
            .collect::<Vec<_>>(),
        "an empty cache says no run was saved"
    );
    assert_eq!(timing.steps.len(), 2);
    assert!(
        timing
            .steps
            .iter()
            .all(|step| step.at_merge_base == Some(true))
    );
    assert!(!crate::flock::is_held(taken).unwrap());
}
