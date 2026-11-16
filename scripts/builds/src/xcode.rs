//! What xcodebuild prints when it cannot start on a Mac. A Mac whose Xcode
//! cannot load a plug-in it needs (as when Xcode was updated and the system
//! content its first launch installs was not) fails every xcodebuild run with
//! the line `CANNOT_START` begins, whatever the code it builds. An app build
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
