//! An app lane: the part of a repo's app builds that shares no build output
//! with the other part. A Mac build (Xcode, the macOS bridge) and an Android
//! build (Gradle, the Android bridge) write to different folders, so each lane
//! has app checkouts of its own, and with them their own locks and a queue: the
//! two never wait on each other. A lane has a set of app checkouts, one lock
//! each, and one queue: the first builds in line take the free locks.
//!
//! In a repo's folder on the volume, the mac lane's files are `app-checkouts`
//! (the set, one checkout per line, `AppCheckout`), `app-checkouts-removed`
//! (the checkouts builds removed from the set lately, `Removed`),
//! `app.locks/<checkout>` and `app.queue/`; the android lane's carry
//! `-android`.

use crate::fail;
use crate::fail::Result;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
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

    /// Whether the lane builds with Xcode, so a host whose Xcode cannot start
    /// (xcode.rs), or whose UI tests time out turning on automation mode
    /// (ui_test_settings.rs), cannot build it.
    pub fn uses_xcode(self) -> bool {
        match self {
            Lane::Mac => true,
            Lane::Android => false,
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

    /// The file of the checkouts builds removed from the lane lately, one
    /// per line, in the repo's folder.
    pub fn removed_file_name(self) -> String {
        format!("app{}-checkouts-removed", self.tag())
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

/// Is <name>, an entry of a repo's folder, one of the lanes' set files,
/// removal records or lock folders?
pub fn is_lane_file(name: &str) -> bool {
    Lane::ALL.into_iter().any(|lane| {
        name == lane.checkouts_file_name()
            || name == lane.removed_file_name()
            || name == lane.locks_dir_name()
    })
}

/// Who put an app checkout in its lane's set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddedBy {
    /// A person, with `builds app-checkout`. The checkout stays until a
    /// person removes it.
    Person,
    /// builds, at <at> (seconds since 1970), for a build that waited, for the
    /// reason <why> (lane_size.rs). builds removes the checkout once no build
    /// has used it for a while.
    Builds { at: i64, why: String },
}

/// One line of a lane's set: the checkout's entry (`side-N` here, or
/// `<host>:<name>` on a host) alone when a person added it, or
/// `<entry> builds <at> <why>` when builds did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppCheckout {
    pub entry: String,
    pub added_by: AddedBy,
}

/// The word between the entry and the time on a line of a checkout builds
/// added.
const BY_BUILDS: &str = "builds";

impl AppCheckout {
    /// A checkout a person added.
    pub fn by_person(entry: &str) -> AppCheckout {
        AppCheckout {
            entry: entry.to_string(),
            added_by: AddedBy::Person,
        }
    }

    pub fn parse(line: &str) -> Result<AppCheckout> {
        let mut words = line.splitn(4, ' ');
        let entry = words.next().unwrap_or("");
        let added_by = match (words.next(), words.next(), words.next()) {
            (None, ..) => AddedBy::Person,
            (Some(BY_BUILDS), Some(at), Some(why)) if !why.is_empty() => match at.parse() {
                Ok(at) => AddedBy::Builds {
                    at,
                    why: why.to_string(),
                },
                Err(_) => fail!("{line:?} is not a line of a lane's app checkouts"),
            },
            _ => fail!("{line:?} is not a line of a lane's app checkouts"),
        };
        if entry.is_empty() {
            fail!("{line:?} is not a line of a lane's app checkouts");
        }
        Ok(AppCheckout {
            entry: entry.to_string(),
            added_by,
        })
    }

    pub fn line(&self) -> String {
        match &self.added_by {
            AddedBy::Person => self.entry.clone(),
            AddedBy::Builds { at, why } => format!("{} {BY_BUILDS} {at} {why}", self.entry),
        }
    }
}

/// The checkouts named in the text of a lane's set file, in the order
/// recorded.
pub fn parse_set(text: &str) -> Result<Vec<AppCheckout>> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(AppCheckout::parse)
        .collect()
}

/// A checkout builds removed from a lane's set: `<at> <entry> <why>` on a line
/// of the lane's removal record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    /// Seconds since 1970.
    pub at: i64,
    pub entry: String,
    pub why: String,
}

impl Removed {
    pub fn parse(line: &str) -> Result<Removed> {
        let mut words = line.splitn(3, ' ');
        match (words.next(), words.next(), words.next()) {
            (Some(at), Some(entry), Some(why)) if !entry.is_empty() && !why.is_empty() => {
                match at.parse() {
                    Ok(at) => Ok(Removed {
                        at,
                        entry: entry.to_string(),
                        why: why.to_string(),
                    }),
                    Err(_) => fail!("{line:?} is not a line of a lane's removed checkouts"),
                }
            }
            _ => fail!("{line:?} is not a line of a lane's removed checkouts"),
        }
    }

    pub fn line(&self) -> String {
        format!("{} {} {}", self.at, self.entry, self.why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_names_its_set_its_locks_and_its_queue() {
        assert_eq!(Lane::Mac.checkouts_file_name(), "app-checkouts");
        assert_eq!(
            Lane::Android.removed_file_name(),
            "app-android-checkouts-removed"
        );
        assert_eq!(Lane::Mac.locks_dir_name(), "app.locks");
        assert_eq!(Lane::Mac.queue_dir_name(), "app.queue");
        assert_eq!(Lane::Android.checkouts_file_name(), "app-android-checkouts");
        assert_eq!(Lane::Android.locks_dir_name(), "app-android.locks");
        assert_eq!(Lane::Android.queue_dir_name(), "app-android.queue");
        assert_eq!(
            Lane::Android.legacy_checkout_file_name(),
            "app-android-checkout"
        );
        assert_eq!(Lane::Mac.legacy_lock_file_name(), "app.lock");
    }

    #[test]
    fn a_lane_is_found_by_its_word() {
        assert_eq!(Lane::parse("android"), Some(Lane::Android));
        assert_eq!(Lane::parse("ios"), None);
        assert!(is_lane_file("app-android.locks") && is_lane_file("app-checkouts"));
        assert!(
            !is_lane_file("app.queue") && !is_lane_file("app-checkout") && !is_lane_file("side-1")
        );
        assert!(is_lane_file("app-checkouts-removed"));
    }

    #[test]
    fn a_set_line_says_who_added_the_checkout() {
        let set =
            parse_set("side-1\nm1:builds-android-1 builds 1763337600 2 builds waited\n").unwrap();
        assert_eq!(set[0], AppCheckout::by_person("side-1"));
        assert_eq!(
            set[1].added_by,
            AddedBy::Builds {
                at: 1763337600,
                why: "2 builds waited".to_string()
            }
        );
        assert_eq!(
            set[1].line(),
            "m1:builds-android-1 builds 1763337600 2 builds waited"
        );
        assert!(AppCheckout::parse("side-1 someone").is_err());
        assert!(AppCheckout::parse("side-1 builds soon why").is_err());
        let removed = Removed::parse("1763337600 m1:builds-mac-1 no build for 61 minutes").unwrap();
        assert_eq!(removed.entry, "m1:builds-mac-1");
        assert_eq!(Removed::parse(&removed.line()).unwrap(), removed);
    }
}
