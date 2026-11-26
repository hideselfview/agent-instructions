//! An app lane: the part of a repo's app builds that shares no build output
//! with the other parts. A lane is one app that the repo's `.app-build` names
//! (`boards::app_build`): a Mac build (Xcode, the macOS bridge) and an Android
//! build (Gradle, the Android bridge) write to different folders, so each app
//! has app checkouts of its own, and with them their own locks and a queue: two
//! apps never wait on each other. A lane has a set of app checkouts, one lock
//! each, and one queue: the first builds in line take the free locks.
//!
//! In a repo's folder on the volume, an app's files are
//! `app-<app>-checkouts` (the set, one checkout per line, `AppCheckout`),
//! `app-<app>-checkouts-removed` (the checkouts builds removed from the set
//! lately, `Removed`), `app-<app>.locks/<checkout>` and `app-<app>.queue/`.

use crate::fail;
use crate::fail::Result;
use boards::app_build::{APP_NAME_BYTES, App, Apps, Platform};

/// An app's name held in place, so that a lane is `Copy`: the name's bytes,
/// then zeros. Names order as their text does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Name {
    bytes: [u8; APP_NAME_BYTES],
    len: u8,
}

impl Name {
    /// <text>, which fits: `boards::app_build` refuses a longer app's name.
    const fn of(text: &str) -> Name {
        let source = text.as_bytes();
        let mut bytes = [0; APP_NAME_BYTES];
        let mut at = 0;
        while at < source.len() {
            bytes[at] = source[at];
            at += 1;
        }
        Name {
            bytes,
            len: source.len() as u8,
        }
    }

    fn text(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("a name holds the text it was made from")
    }
}

/// One lane: an app of a repo, by its name and its platform. Lanes order by
/// platform, then name, as an `App` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lane {
    platform: Platform,
    name: Name,
}

/// A lane keeps its app's name and platform in JSON, as one word: the name
/// alone for an app named for its platform (`mac`), else
/// `<name>:<platform>`. What a page shows is `Lane::word`.
impl serde::Serialize for Lane {
    fn serialize<S: serde::Serializer>(&self, to: S) -> std::result::Result<S::Ok, S::Error> {
        if Lane::named_for(self.word()) == Some(*self) {
            to.serialize_str(self.word())
        } else {
            to.serialize_str(&format!("{}:{}", self.word(), self.platform.word()))
        }
    }
}

impl<'de> serde::Deserialize<'de> for Lane {
    fn deserialize<D: serde::Deserializer<'de>>(from: D) -> std::result::Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(from)?;
        let lane = match text.split_once(':') {
            None => Lane::named_for(&text),
            Some((name, platform)) => Platform::parse(platform)
                .filter(|_| boards::app_build::is_app_name(name))
                .map(|platform| Lane {
                    platform,
                    name: Name::of(name),
                }),
        };
        lane.ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not a lane")))
    }
}

impl Lane {
    /// The app named `mac`, which runs on the mac platform.
    pub const MAC: Lane = Lane {
        platform: Platform::Mac,
        name: Name::of("mac"),
    };
    /// The app named `android`, which runs on the android platform.
    pub const ANDROID: Lane = Lane {
        platform: Platform::Android,
        name: Name::of("android"),
    };
    /// The app named `web`, which runs on the web platform.
    pub const WEB: Lane = Lane {
        platform: Platform::Web,
        name: Name::of("web"),
    };

    /// The lane that builds <app>.
    pub fn of(app: &App) -> Lane {
        Lane {
            platform: app.platform,
            name: Name::of(&app.name),
        }
    }

    /// The app named for a platform, when <word> is a platform's word. Such
    /// an app needs no `.app-build` line, and no line gives its name to an
    /// app on another platform.
    pub fn named_for(word: &str) -> Option<Lane> {
        match Platform::parse(word)? {
            Platform::Mac => Some(Lane::MAC),
            Platform::Android => Some(Lane::ANDROID),
            Platform::Web => Some(Lane::WEB),
        }
    }

    /// The lane that <apps> (a repo's `app` lines) gives <word>, or none.
    pub fn parse_in(apps: &Apps, word: &str) -> Option<Lane> {
        apps.get(word).map(|app| Lane::of(&app))
    }

    /// Where the app word <word> goes in a list of words that a store holds
    /// with no platform: the apps named for platforms first, in the
    /// platforms' order, then every other app by name.
    pub fn word_order(word: &str) -> (bool, Option<Lane>, &str) {
        let named = Lane::named_for(word);
        (named.is_none(), named, word)
    }

    /// `parse_in`, failing with the apps there are.
    pub fn parse_in_or_fail(apps: &[Lane], word: &str) -> Result<Lane> {
        match apps.iter().find(|lane| lane.word() == word) {
            Some(lane) => Ok(*lane),
            None => fail!(
                "{word:?} is no app of this repo's .app-build ({})",
                apps.iter()
                    .map(|lane| lane.word())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
        }
    }

    /// The app's name.
    pub fn word(&self) -> &str {
        self.name.text()
    }

    /// The shots prefix that every shot name of the lane starts with
    /// (`mac.`): the prefix of a run of the lane's full set of shots.
    pub fn all_shots_prefix(self) -> String {
        format!("{}.", self.word())
    }

    /// The platform whose shots this lane builds, and whose row of the
    /// platform table (app_platform.rs) holds the lane's facts about its host.
    pub fn platform(self) -> Platform {
        self.platform
    }

    /// The app the lane builds.
    pub fn app(self) -> App {
        App {
            platform: self.platform,
            name: self.word().to_string(),
        }
    }

    /// The file naming the lane's app checkouts, one per line, in the repo's
    /// folder.
    pub fn checkouts_file_name(self) -> String {
        format!("app-{}-checkouts", self.word())
    }

    /// The file of the checkouts builds removed from the lane lately, one
    /// per line, in the repo's folder.
    pub fn removed_file_name(self) -> String {
        format!("app-{}-checkouts-removed", self.word())
    }

    /// The folder of the lane's locks, one file per app checkout, in the
    /// repo's folder.
    pub fn locks_dir_name(self) -> String {
        format!("app-{}.locks", self.word())
    }

    /// The queue folder, in the repo's folder.
    pub fn queue_dir_name(self) -> String {
        format!("app-{}.queue", self.word())
    }
}

/// Is <name>, an entry of a repo's folder, one of the lanes' set files,
/// removal records or lock folders?
pub fn is_lane_file(name: &str) -> bool {
    app_of_file(name).is_some()
}

/// The app that <name>, an entry of a repo's folder, is a set file, removal
/// record or lock folder of.
pub fn app_of_file(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("app-")?;
    rest.strip_suffix("-checkouts-removed")
        .or_else(|| rest.strip_suffix("-checkouts"))
        .or_else(|| rest.strip_suffix(".locks"))
        .filter(|app| boards::app_build::is_app_name(app))
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
        assert_eq!(Lane::MAC.checkouts_file_name(), "app-mac-checkouts");
        assert_eq!(
            Lane::ANDROID.removed_file_name(),
            "app-android-checkouts-removed"
        );
        assert_eq!(Lane::MAC.locks_dir_name(), "app-mac.locks");
        assert_eq!(Lane::MAC.queue_dir_name(), "app-mac.queue");
        assert_eq!(Lane::ANDROID.checkouts_file_name(), "app-android-checkouts");
        assert_eq!(Lane::ANDROID.locks_dir_name(), "app-android.locks");
        assert_eq!(Lane::ANDROID.queue_dir_name(), "app-android.queue");
    }

    #[test]
    fn a_lane_is_an_app_with_a_platform() {
        let ios = Lane::of(&App {
            platform: Platform::Mac,
            name: "zeta-shell".to_string(),
        });
        assert_eq!(ios.word(), "zeta-shell");
        assert_eq!(ios.platform(), Platform::Mac);
        assert_eq!(ios.checkouts_file_name(), "app-zeta-shell-checkouts");
        assert_eq!(Lane::named_for("zeta-shell"), None);
        for lane in [ios, Lane::MAC] {
            let said = serde_json::to_string(&lane).unwrap();
            assert_eq!(serde_json::from_str::<Lane>(&said).unwrap(), lane);
        }
        assert_eq!(serde_json::to_string(&Lane::MAC).unwrap(), "\"mac\"");
        assert_eq!(Lane::of(&ios.app()), ios);
        assert_eq!(Lane::named_for("web"), Some(Lane::WEB));
        assert!(Lane::MAC < ios && ios < Lane::ANDROID);
    }

    #[test]
    fn a_lane_is_found_by_its_word() {
        assert_eq!(Lane::named_for("android"), Some(Lane::ANDROID));
        assert_eq!(Lane::named_for("ios"), None);
        assert!(is_lane_file("app-android.locks") && is_lane_file("app-mac-checkouts"));
        assert!(
            !is_lane_file("app.queue")
                && !is_lane_file("app-mac.queue")
                && !is_lane_file("app-checkout")
                && !is_lane_file("side-1")
        );
        assert!(is_lane_file("app-mac-checkouts-removed"));
        assert_eq!(app_of_file("app-web-checkouts"), Some("web"));
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
