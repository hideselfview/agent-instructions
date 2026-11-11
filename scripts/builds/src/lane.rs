//! An app lane: the part of a repo's app builds that shares no build output
//! with the other part. A Mac build (Xcode, the macOS bridge) and an Android
//! build (Gradle, the Android bridge) write to different folders, so each lane
//! has an app checkout of its own, and with it its own lock and queue: the two
//! never wait on each other.
//!
//! The mac lane keeps the names the one app checkout had before there were
//! lanes (`app-checkout`, `app.lock`, `app.queue`), so a build started by an
//! older `builds` and one started by this one are on the same lock; the
//! android lane's carry `-android`.

use crate::fail;
use crate::fail::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lane {
    Mac,
    Android,
}

impl Lane {
    pub const ALL: [Lane; 2] = [Lane::Mac, Lane::Android];

    pub fn word(self) -> &'static str {
        match self {
            Lane::Mac => "mac",
            Lane::Android => "android",
        }
    }

    pub fn parse(word: &str) -> Option<Lane> {
        Lane::ALL.into_iter().find(|lane| lane.word() == word)
    }

    /// `parse`, failing with the words there are.
    pub fn parse_or_fail(word: &str) -> Result<Lane> {
        match Lane::parse(word) {
            Some(lane) => Ok(lane),
            None => fail!("{word:?} is not a lane (mac | android)"),
        }
    }

    /// What the lane's file names carry between `app` and the extension: the
    /// mac lane's carry nothing.
    fn tag(self) -> &'static str {
        match self {
            Lane::Mac => "",
            Lane::Android => "-android",
        }
    }

    /// The file naming the lane's app checkout, in the repo's folder.
    pub fn checkout_file_name(self) -> String {
        format!("app{}-checkout", self.tag())
    }

    /// The lock file, in the repo's folder.
    pub fn lock_file_name(self) -> String {
        format!("app{}.lock", self.tag())
    }

    /// The queue folder, in the repo's folder.
    pub fn queue_dir_name(self) -> String {
        format!("app{}.queue", self.tag())
    }
}

/// Is <name>, an entry of a repo's folder, one of the lanes' checkout or lock
/// files?
pub fn is_lane_file(name: &str) -> bool {
    Lane::ALL.into_iter().any(|lane| name == lane.checkout_file_name() || name == lane.lock_file_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_lane_keeps_the_names_from_before_there_were_lanes() {
        assert_eq!(Lane::Mac.checkout_file_name(), "app-checkout");
        assert_eq!(Lane::Mac.lock_file_name(), "app.lock");
        assert_eq!(Lane::Mac.queue_dir_name(), "app.queue");
        assert_eq!(Lane::Android.checkout_file_name(), "app-android-checkout");
        assert_eq!(Lane::Android.lock_file_name(), "app-android.lock");
        assert_eq!(Lane::Android.queue_dir_name(), "app-android.queue");
    }

    #[test]
    fn a_lane_is_found_by_its_word() {
        assert_eq!(Lane::parse("android"), Some(Lane::Android));
        assert_eq!(Lane::parse("ios"), None);
        assert!(is_lane_file("app-android.lock") && is_lane_file("app-checkout"));
        assert!(!is_lane_file("app.queue") && !is_lane_file("side-1"));
    }
}
