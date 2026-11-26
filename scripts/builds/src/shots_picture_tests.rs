use super::*;
use crate::shots_cache::{Key, stage};
use crate::testing::Scratch;

fn key(prefix: &str, commit: &str) -> Key {
    Key {
        repo: "demo".to_string(),
        commit: commit.to_string(),
        lane: Lane::Mac,
        environment: "26B131-17C529".to_string(),
        prefix: prefix.to_string(),
        versions: versions(),
    }
}

fn versions() -> Versions {
    Versions {
        capture: "c0c0c0c0c0c0c0c0".to_string(),
        comparison: "0123456789abcdef".to_string(),
    }
}

/// Caches a run of <key> whose report holds the pictures <files>.
fn cache_run(places: &Places, key: &Key, files: &[&str]) {
    let staging = stage(places, key).unwrap();
    for file in files {
        let path = staging.report().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "png").unwrap();
    }
    std::fs::write(staging.report().join(crate::shot_causes::FILE), "{}").unwrap();
    staging.finish(0, "ok\n").unwrap();
}

fn ask(places: &Places, commit: &str, scene: &str, side: Side) -> Option<PathBuf> {
    stored(places, "demo", commit, Lane::Mac, scene, side, &versions()).unwrap()
}

#[test]
fn a_run_that_covers_the_scene_hands_back_its_board_and_its_shot() {
    let scratch = Scratch::new("picture-hit");
    let places = scratch.places();
    let commit = "a".repeat(40);
    cache_run(
        &places,
        &key("mac.", &commit),
        &[
            "img/mac.welcome.library-details.board.png",
            "img/mac.welcome.library-details.shot.png",
        ],
    );
    for side in [Side::Board, Side::Shot] {
        let picture = ask(&places, &commit, "mac.welcome.library-details", side).unwrap();
        assert!(picture.ends_with(format!(
            "img/mac.welcome.library-details.{}.png",
            side.word()
        )));
        assert!(picture.is_file());
    }
}

#[test]
fn a_run_without_the_picture_hands_back_none() {
    let scratch = Scratch::new("picture-miss");
    let places = scratch.places();
    let commit = "a".repeat(40);
    cache_run(
        &places,
        &key("mac.welcome.", &commit),
        &["img/mac.welcome.library-details.board.png"],
    );
    assert!(ask(&places, &commit, "mac.welcome.library-details", Side::Shot).is_none());
    assert!(ask(&places, &commit, "mac.welcome.other", Side::Board).is_none());
    assert!(
        ask(
            &places,
            &"b".repeat(40),
            "mac.welcome.library-details",
            Side::Board
        )
        .is_none()
    );
}

#[test]
fn a_run_of_another_prefix_does_not_cover_the_scene() {
    let scratch = Scratch::new("picture-prefix");
    let places = scratch.places();
    let commit = "a".repeat(40);
    cache_run(
        &places,
        &key("mac.queue.", &commit),
        &["img/mac.welcome.library-details.board.png"],
    );
    assert!(ask(&places, &commit, "mac.welcome.library-details", Side::Board).is_none());
}

#[test]
fn the_scene_names_its_lane_and_a_side_is_board_or_shot() {
    assert_eq!(
        lane_of("android.playlists.playlist").unwrap(),
        Lane::Android
    );
    assert_eq!(lane_of("mac.welcome.library-details").unwrap(), Lane::Mac);
    assert!(
        lane_of("web.landing.home")
            .unwrap_err()
            .text()
            .contains("web")
    );
    assert!(lane_of("welcome").is_err());
    assert_eq!(Side::parse("board").unwrap(), Side::Board);
    assert!(
        Side::parse("both")
            .unwrap_err()
            .text()
            .contains("board | shot")
    );
}

#[test]
fn a_missing_picture_is_asked_of_an_app_build_of_the_scene_that_runs_again() {
    assert_eq!(
        app_build_words("bae", "origin/main", "mac.welcome.library-details"),
        [
            "bae",
            "origin/main",
            "--shots",
            "mac.welcome.library-details",
            "--fresh"
        ]
    );
}
