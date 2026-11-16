//! What xcodebuild prints when it cannot start on a Mac. When someone updates
//! Xcode and Xcode's first launch has not installed its system content yet,
//! Xcode cannot load a plug-in it needs, and every xcodebuild run fails with a
//! line that starts with `CANNOT_START`, whatever code it builds. An app build
//! that sees that line in a failed step's output counts the failure as the
//! host's, not the code's (app_build.rs).

/// The start of the line xcodebuild prints when a plug-in it needs fails to
/// load. The line goes on to name the command that fixes it.
pub const CANNOT_START: &str = "A required plugin failed to load";

/// The command xcodebuild names in <said>, the line it printed, as the fix:
/// the words quoted after "try running".
pub fn fix_named_in(said: &str) -> Option<&str> {
    let (_, after) = said.split_once("try running '")?;
    let (fix, _) = after.split_once('\'')?;
    Some(fix).filter(|fix| !fix.is_empty())
}

#[cfg(test)]
#[path = "xcode_tests.rs"]
mod tests;
