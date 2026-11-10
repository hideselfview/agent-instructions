//! Where everything the tool owns or tidies is, and the rules it keeps there,
//! read once from the environment. Tests build their own, in temp folders.

use std::path::PathBuf;

use crate::fail::Result;
use crate::fail;

/// A file in a build folder written this recently may belong to a build
/// between two of its steps, holding no lock or open file: the floor, the
/// DerivedData cap, the shared caches' caps and the temporary folders leave
/// it alone.
pub const FLOOR_IDLE_MINUTES: u64 = 30;
/// Xcode's default DerivedData location (`install` sets it). 15G holds five
/// one-off entries of about 3 GB, more than the checkouts that run one-offs at
/// once (main, the app checkout, a side worktree or two); before it had a
/// cap, nine entries had piled up to 27 GB, four from worktrees already
/// removed.
pub const DERIVED_DATA_CAP_KIB: u64 = 15 * 1024 * 1024;
/// `prune` deletes a DerivedData entry nothing wrote to for this long: a day
/// of work keeps what that day's one-off runs resolved.
pub const DERIVED_DATA_IDLE_MINUTES: u64 = 1440;
/// A free side worktree nothing has used for this long is retired by `prune`.
pub const FREE_WORKTREE_IDLE_MINUTES: u64 = 1440;
/// The port `dashboard` serves on unless `--port` names another.
pub const DASHBOARD_PORT: u16 = 7787;

/// macOS's own entries at the root of a volume. Nothing builds there, and
/// most are closed to the user.
pub const MACOS_ENTRIES: [&str; 8] = [
    ".DS_Store",
    ".Spotlight-V100",
    ".fseventsd",
    ".TemporaryItems",
    ".Trashes",
    ".DocumentRevisions-V100",
    ".VolumeIcon.icns",
    ".com.apple.timemachine.donotpresent",
];

/// Is <name>, an entry at the volume's root, one the daemon doesn't follow?
/// macOS's own entries, and the trash, which holds only folders being
/// deleted: following it would apply each of their files' deletions, 100k
/// for one cargo folder, to answer questions nobody asks.
pub fn unfollowed(name: &str) -> bool {
    MACOS_ENTRIES.contains(&name) || name == ".trash"
}

#[derive(Clone, Debug)]
pub struct Places {
    /// The build volume, `/Volumes/Builds` (BUILDS_ROOT).
    pub root: PathBuf,
    pub home: PathBuf,
    /// CARGO_HOME, or ~/.cargo.
    pub cargo_home: PathBuf,
    /// /private/tmp, where probes and stray builds land.
    pub private_tmp: PathBuf,
    /// `getconf DARWIN_USER_TEMP_DIR`, where mktemp puts things.
    pub user_temp: Option<PathBuf>,
    /// This session's TMPDIR.
    pub tmpdir: Option<PathBuf>,
    pub uid: u32,
}

impl Places {
    pub fn from_env() -> Result<Places> {
        let home = match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home),
            None => fail!("HOME is not set"),
        };
        let root = std::env::var_os("BUILDS_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Volumes/Builds"));
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cargo"));
        let tmpdir = std::env::var_os("TMPDIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Ok(Places {
            root,
            home,
            cargo_home,
            private_tmp: PathBuf::from("/private/tmp"),
            user_temp: darwin_user_temp_dir(),
            tmpdir,
            uid: unsafe { libc::getuid() },
        })
    }

    pub fn trash(&self) -> PathBuf {
        self.root.join(".trash")
    }

    pub fn derived_data(&self) -> PathBuf {
        self.root.join("DerivedData")
    }

    pub fn caches(&self) -> PathBuf {
        self.root.join("caches")
    }

    /// The file naming <repo>'s app checkout, the one side worktree that keeps
    /// app builds warm beside main; `builds app-checkout` writes it. It sits
    /// beside the app checkout's lock, <root>/<repo>/app.lock.
    pub fn app_checkout_file(&self, repo: &str) -> PathBuf {
        self.root.join(repo).join("app-checkout")
    }

    /// <repo>'s app checkout name (side-N), when one is recorded.
    pub fn app_checkout_of(&self, repo: &str) -> Result<Option<String>> {
        let file = self.app_checkout_file(repo);
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(Some(text.trim_end_matches('\n').to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => fail!("reading {}: {error}", file.display()),
        }
    }

    /// Claude Code sessions' scratch folders: /private/tmp/claude-<uid>.
    pub fn scratch_base(&self) -> PathBuf {
        self.private_tmp.join(format!("claude-{}", self.uid))
    }

    /// Claude Code's record of its running sessions: one <pid>.json each,
    /// naming the session's id.
    pub fn claude_sessions(&self) -> PathBuf {
        self.home.join(".claude/sessions")
    }

    /// The daemon's own folder: its socket, and the marks it writes to know
    /// FSEvents has delivered everything before a question.
    pub fn daemon_dir(&self) -> PathBuf {
        self.root.join(".builds-daemon")
    }

    /// The socket the daemon answers on; one daemon per root.
    pub fn daemon_socket(&self) -> PathBuf {
        self.daemon_dir().join("socket")
    }

    pub fn require_volume(&self) -> Result<()> {
        if !self.root.is_dir() {
            fail!("build volume {} is not mounted", self.root.display());
        }
        Ok(())
    }

    /// Is <dir> one of the volume's own folders (DerivedData, caches, the
    /// trash), which a repo can't be named after?
    pub fn is_reserved_name(&self, name: &str) -> bool {
        name == "DerivedData" || name == "caches" || name == "mode" || name.starts_with('.')
    }
}

fn darwin_user_temp_dir() -> Option<PathBuf> {
    // _CS_DARWIN_USER_TEMP_DIR
    const NAME: libc::c_int = 65537;
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    let length = unsafe { libc::confstr(NAME, buffer.as_mut_ptr().cast(), buffer.len()) };
    if length == 0 || length > buffer.len() {
        return None;
    }
    buffer.truncate(length - 1);
    let text = String::from_utf8(buffer).ok()?;
    Some(PathBuf::from(text.trim_end_matches('/')))
}

/// Under the free-space floor, an incremental folder nothing wrote anywhere
/// inside for this long is deleted, in every checkout, claimed ones, main's
/// and the app checkout's too, before any whole folder. It is far shorter
/// than `Rules::incremental_hours`, which bounds a folder's growth while space
/// is plentiful and so keeps a day's working state; under the floor the
/// alternative is a whole folder's cold rebuild of every dependency, a
/// warning, or a stopped build, and an incremental folder costs only an
/// incremental rebuild of its crate. An hour is the gap the floor already
/// grants an agent between builds while it reads and edits, so the crates of
/// its current edit loop stay warm. On 2026-11-10, with eight agents
/// building, the volume sat under the floor for hours while claimed worktrees
/// held 18–25 GB each of incremental folders nothing had written for an hour;
/// deleting those freed 37–48 GB at a time.
pub const FLOOR_INCREMENTAL_IDLE_MINUTES: u64 = 60;

/// What setup, prune and the floor go by.
#[derive(Clone, Debug)]
pub struct Rules {
    /// Under this a setup deletes idle folders and then warns. The free space
    /// a build may need: bae's iOS build is the largest single platform build
    /// (target-ios 12.2G + bae-ios/.build 4.9G fresh); another checkout
    /// building macOS at the same time (bridge 7.4G + DerivedData 5.2G) is
    /// the headroom.
    pub floor_kib: u64,
    /// Under this a build fails instead of warning: the space left is about
    /// one platform build, and the main disk shares the pool, so running out
    /// would break the person's own apps too, not only the build.
    pub hard_floor_kib: u64,
    /// A build copy no build used for this many days is deleted. A coven pin
    /// bump in bae makes a new copy of everything above coven; three days
    /// keeps the copies a branch from the last few days still builds against.
    /// Before it, main's target held 93 copies of bae-core, 13 GB, going back
    /// a week.
    pub unused_days: u64,
    /// An incremental folder nothing wrote anywhere inside for this many
    /// hours is deleted in main, the one checkout whose builds compile
    /// incrementally; in the others every one goes. Cargo never deletes a
    /// folder of an old hash, so they
    /// fill the cap: one coven checkout had 112 of 162 folders, most of 28 GB,
    /// unwritten for two hours. Losing one costs an incremental rebuild of its
    /// crate.
    pub incremental_hours: u64,
}

impl Rules {
    pub fn from_env() -> Result<Rules> {
        Ok(Rules {
            floor_kib: gib_from_env("BUILDS_FREE_FLOOR_GIB", 30)? * 1024 * 1024,
            hard_floor_kib: gib_from_env("BUILDS_HARD_FLOOR_GIB", 10)? * 1024 * 1024,
            unused_days: whole_from_env("BUILDS_UNUSED_DAYS", 3, "days")?,
            incremental_hours: whole_from_env("BUILDS_INCREMENTAL_HOURS", 6, "hours")?,
        })
    }
}

fn whole_from_env(name: &str, default: u64, unit: &str) -> Result<u64> {
    let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    let text = value.to_string_lossy();
    match text.parse::<u64>() {
        Ok(n) if n >= 1 && text.bytes().all(|b| b.is_ascii_digit()) => Ok(n),
        _ => fail!("{name} must be a whole number of {unit}, at least 1, got '{text}'"),
    }
}

fn gib_from_env(name: &str, default: u64) -> Result<u64> {
    let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    let text = value.to_string_lossy();
    match text.parse::<u64>() {
        Ok(n) if text.bytes().all(|b| b.is_ascii_digit()) => Ok(n),
        _ => fail!("{name} must be a whole number of GiB, got '{text}'"),
    }
}
