use super::*;

/// The line an M1 whose Xcode was updated without its system content
/// printed in `xcodebuild -create-xcframework`.
const SAID: &str = "A required plugin failed to load. Please ensure system content is up-to-date \u{2014} try running 'xcodebuild -runFirstLaunch'.";

#[test]
fn the_fix_is_the_command_xcodebuild_quotes() {
    assert!(SAID.starts_with(CANNOT_START));
    assert_eq!(fix_named_in(SAID), Some("xcodebuild -runFirstLaunch"));
    assert_eq!(fix_named_in("A required plugin failed to load."), None);
    assert_eq!(fix_named_in("try running ''"), None);
}
