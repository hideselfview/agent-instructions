use super::*;
use crate::testing::Scratch;

fn checkout(top: &str, main: &str) -> Checkout {
    Checkout {
        top: PathBuf::from(top),
        main: PathBuf::from(main),
        git_dir: PathBuf::from(main).join(".git"),
    }
}

#[test]
fn the_settings_turn_caching_on_with_the_hosts_store_and_map_the_checkouts_folders() {
    let scratch = Scratch::new("xcode-cache-config");
    let places = scratch.places();
    let side = checkout("/work/demo-worktrees/side-1", "/work/demo");
    let file = ensure(&places, &side, true).unwrap();
    assert_eq!(
        file,
        places.root.join(".xcode-settings/demo/side-1.xcconfig")
    );
    let text = std::fs::read_to_string(&file).unwrap();
    let root = places.root.display();
    let maps = format!("/work/demo-worktrees/side-1=/^checkout {root}/demo/side-1=/^build");
    assert_eq!(
        text,
        format!(
            "COMPILATION_CACHE_ENABLE_CACHING = YES\n\
             COMPILATION_CACHE_CAS_PATH = {root}/caches/xcode-cas\n\
             SWIFT_ENABLE_PREFIX_MAPPING = YES\n\
             CLANG_ENABLE_PREFIX_MAPPING = YES\n\
             SWIFT_OTHER_PREFIX_MAPPINGS = {maps}\n\
             CLANG_OTHER_PREFIX_MAPPINGS = {maps}\n"
        )
    );
    assert!(places.caches().join("xcode-cas").is_dir());
}

#[test]
fn two_checkouts_of_a_repo_get_the_same_mapped_names_and_the_same_store() {
    let scratch = Scratch::new("xcode-cache-two");
    let places = scratch.places();
    let one = std::fs::read_to_string(
        ensure(
            &places,
            &checkout("/work/demo-worktrees/a", "/work/demo"),
            true,
        )
        .unwrap(),
    )
    .unwrap();
    let two = std::fs::read_to_string(
        ensure(
            &places,
            &checkout("/work/demo-worktrees/b", "/work/demo"),
            true,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(one.contains("=/^checkout ") && one.contains("=/^build"));
    assert!(two.contains("=/^checkout ") && two.contains("=/^build"));
    let store = |text: &str| {
        text.lines()
            .find(|line| line.starts_with("COMPILATION_CACHE_CAS_PATH"))
            .unwrap()
            .to_string()
    };
    assert_eq!(store(&one), store(&two));
}

#[test]
fn a_changed_settings_file_is_written_again_and_a_current_one_is_left() {
    let scratch = Scratch::new("xcode-cache-rewrite");
    let places = scratch.places();
    let side = checkout("/work/demo-worktrees/side-1", "/work/demo");
    let file = ensure(&places, &side, true).unwrap();
    std::fs::write(&file, "stale\n").unwrap();
    ensure(&places, &side, true).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .starts_with("COMPILATION_CACHE_ENABLE_CACHING")
    );
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    ensure(&places, &side, true).unwrap();
    assert_eq!(
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        before
    );
}

#[test]
fn a_path_with_a_space_fails_instead_of_mapping_wrongly() {
    let scratch = Scratch::new("xcode-cache-space");
    let places = scratch.places();
    let Err(error) = ensure(
        &places,
        &checkout("/my work/demo-worktrees/a", "/my work/demo"),
        true,
    ) else {
        panic!("a path with a space has no mapping");
    };
    let text = error.text();
    assert!(text.contains("has a space"), "{text}");
}

#[test]
fn the_cache_is_a_shared_cache_with_a_cap_and_no_link() {
    let cache = crate::manifest::SHARED_CACHES
        .iter()
        .find(|cache| cache.name == CAS)
        .expect("listed");
    assert!(cache.link.is_none() && cache.cap_kib > 0);
}

#[test]
fn an_xcode_older_than_the_first_that_maps_gets_caching_without_mapping() {
    let scratch = Scratch::new("xcode-cache-old");
    let places = scratch.places();
    let side = checkout("/work/demo-worktrees/side-1", "/work/demo");
    let text = std::fs::read_to_string(ensure(&places, &side, false).unwrap()).unwrap();
    assert!(text.contains("COMPILATION_CACHE_ENABLE_CACHING = YES"));
    assert!(!text.contains("PREFIX_MAPPING"));
}

#[test]
fn the_version_is_read_from_the_first_line_of_xcodebuilds_answer() {
    assert_eq!(
        parse_xcode_version("Xcode 27.2\nBuild version 27B52\n"),
        Some((27, 2))
    );
    assert_eq!(parse_xcode_version("Xcode 26\n"), Some((26, 0)));
    assert_eq!(parse_xcode_version("xcode-select: error\n"), None);
    assert_eq!(parse_xcode_version(""), None);
    assert!((26, 3) < MAPPING_FROM && (27, 2) >= MAPPING_FROM && (28, 0) >= MAPPING_FROM);
}

#[test]
fn a_folder_in_no_checkout_has_no_settings_file() {
    let scratch = Scratch::new("xcode-cache-no-checkout");
    let places = scratch.places();
    let Err(error) = settings_for(&places, &scratch.system(), &scratch.dir) else {
        panic!("a folder in no checkout has no settings");
    };
    assert!(error.text().contains("is in no git checkout"));
}
