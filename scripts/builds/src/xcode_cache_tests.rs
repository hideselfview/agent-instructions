use super::*;
use crate::testing::Scratch;

#[test]
fn the_settings_turn_caching_on_with_the_hosts_store() {
    let scratch = Scratch::new("xcode-cache-config");
    let places = scratch.places();
    let file = ensure(&places).unwrap();
    assert_eq!(file, places.root.join(".xcode-compilation-cache.xcconfig"));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("COMPILATION_CACHE_ENABLE_CACHING = YES"));
    assert!(text.contains(&format!(
        "COMPILATION_CACHE_CAS_PATH = {}/caches/xcode-cas\n",
        places.root.display()
    )));
    assert!(places.caches().join("xcode-cas").is_dir());
}

#[test]
fn a_changed_settings_file_is_written_again_and_a_current_one_is_left() {
    let scratch = Scratch::new("xcode-cache-rewrite");
    let places = scratch.places();
    let file = ensure(&places).unwrap();
    std::fs::write(&file, "stale\n").unwrap();
    ensure(&places).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .starts_with("COMPILATION_CACHE_ENABLE_CACHING")
    );
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    ensure(&places).unwrap();
    assert_eq!(
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        before
    );
}

#[test]
fn the_cache_is_a_shared_cache_with_a_cap_and_no_link() {
    let cache = crate::manifest::SHARED_CACHES
        .iter()
        .find(|cache| cache.name == CAS)
        .expect("listed");
    assert!(cache.link.is_none() && cache.cap_kib > 0);
}
