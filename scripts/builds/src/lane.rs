//! An app lane: the part of a repo's app builds that shares no build output
//! with the other part. A Mac build (Xcode, the macOS bridge) and an Android
//! build (Gradle, the Android bridge) write to different folders, so each lane
//! has app checkouts of its own, and with them their own locks and a queue: the
//! two never wait on each other. A lane has a set of app checkouts, one lock
//! each, and one queue: the first builds in line take the free locks.
//!
//! In a repo's folder on the volume, the mac lane's files are `app-checkouts`
//! (the set, one side worktree name per line), `app.locks/<checkout>` and
//! `app.queue/`; the android lane's carry `-android`.

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

    /// The file naming the lane's app checkouts, one per line, in the repo's
    /// folder.
    pub fn checkouts_file_name(self) -> String {
        format!("app{}-checkouts", self.tag())
    }

    /// The folder of the lane's locks, one file per app checkout, in the
    /// repo's folder.
    pub fn locks_dir_name(self) -> String {
        format!("app{}.locks", self.tag())
    }

    /// What the file naming the lane's one app checkout was called before a
    /// lane had a set (`migrate_app_files` moves it).
    pub fn legacy_checkout_file_name(self) -> String {
        format!("app{}-checkout", self.tag())
    }

    /// What the lane's one lock was called before a lane had a set.
    pub fn legacy_lock_file_name(self) -> String {
        format!("app{}.lock", self.tag())
    }

    /// The queue folder, in the repo's folder.
    pub fn queue_dir_name(self) -> String {
        format!("app{}.queue", self.tag())
    }
}

/// Is <name>, an entry of a repo's folder, one of the lanes' set files or lock
/// folders?
pub fn is_lane_file(name: &str) -> bool {
    Lane::ALL.into_iter().any(|lane| name == lane.checkouts_file_name() || name == lane.locks_dir_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_names_its_set_its_locks_and_its_queue() {
        assert_eq!(Lane::Mac.checkouts_file_name(), "app-checkouts");
        assert_eq!(Lane::Mac.locks_dir_name(), "app.locks");
        assert_eq!(Lane::Mac.queue_dir_name(), "app.queue");
        assert_eq!(Lane::Android.checkouts_file_name(), "app-android-checkouts");
        assert_eq!(Lane::Android.locks_dir_name(), "app-android.locks");
        assert_eq!(Lane::Android.queue_dir_name(), "app-android.queue");
        assert_eq!(Lane::Android.legacy_checkout_file_name(), "app-android-checkout");
        assert_eq!(Lane::Mac.legacy_lock_file_name(), "app.lock");
    }

    #[test]
    fn a_lane_is_found_by_its_word() {
        assert_eq!(Lane::parse("android"), Some(Lane::Android));
        assert_eq!(Lane::parse("ios"), None);
        assert!(is_lane_file("app-android.locks") && is_lane_file("app-checkouts"));
        assert!(!is_lane_file("app.queue") && !is_lane_file("app-checkout") && !is_lane_file("side-1"));
    }
}
