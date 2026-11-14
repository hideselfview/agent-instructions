//! Temp places for tests: a scratch volume, temporary folders and git
//! checkouts, all under one folder removed when the test ends.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::places::{Places, Rules};
use crate::tool::Tool;

pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        let base = std::fs::canonicalize(std::env::temp_dir()).expect("the temp dir resolves");
        let dir = base.join(format!("builds-test-{name}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("an old scratch folder is removable");
        }
        std::fs::create_dir_all(&dir).expect("the scratch folder is creatable");
        Scratch { dir }
    }

    /// Places that all point inside the scratch folder.
    pub fn places(&self) -> Places {
        let root = self.dir.join("volume");
        let private_tmp = self.dir.join("tmp");
        for dir in [&root, &private_tmp] {
            std::fs::create_dir_all(dir).expect("creatable");
        }
        Places {
            root,
            home: self.dir.join("home"),
            cargo_home: self.dir.join("cargo"),
            private_tmp,
            user_temp: None,
            tmpdir: None,
            uid: unsafe { libc::getuid() },
            lists: self.dir.join("lists"),
        }
    }

    pub fn tool(&self, floor_gib: u64) -> Tool {
        Tool::new(
            self.places(),
            Rules {
                floor_kib: floor_gib * 1024 * 1024,
                hard_floor_kib: 0,
                unused_days: 3,
                incremental_hours: 6,
            },
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.dir) {
            eprintln!("leaving {}: {error}", self.dir.display());
        }
    }
}

pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        status.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// A repo at <dir> with one commit holding <manifest> as its .build-dirs,
/// and a .gitignore listing its paths, as a repo's own does.
pub fn repo(dir: &Path, manifest: &str) {
    std::fs::create_dir_all(dir).expect("creatable");
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "test"]);
    std::fs::write(dir.join(".build-dirs"), manifest).expect("writable");
    let ignored: Vec<&str> = manifest
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    std::fs::write(dir.join(".gitignore"), ignored.join("\n") + "\n").expect("writable");
    git(dir, &["add", ".build-dirs", ".gitignore"]);
    git(dir, &["commit", "-q", "-m", "Start"]);
}

/// Writes <kib> KiB of incompressible bytes to <path>.
pub fn fill(path: &Path, kib: usize) {
    std::fs::create_dir_all(path.parent().expect("has a folder")).expect("creatable");
    let mut bytes = vec![0u8; kib * 1024];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15 ^ kib as u64;
    for byte in bytes.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    std::fs::write(path, bytes).expect("writable");
}

/// Sets <path>'s modification time <hours> back.
pub fn age(path: &Path, hours: u64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(hours * 3600);
    let file = std::fs::File::open(path).expect("openable");
    file.set_modified(when).expect("settable");
}

/// Sets the time of <dir> and everything in it <hours> back.
pub fn age_all(dir: &Path, hours: u64) {
    for entry in std::fs::read_dir(dir).expect("listable") {
        let path = entry.expect("listable").path();
        if path.is_dir() && !path.is_symlink() {
            age_all(&path, hours);
        } else if !path.is_symlink() {
            age(&path, hours);
        }
    }
    age(dir, hours);
}

/// A socket file at <path> that nothing listens on, as a daemon leaves it
/// after it exits: connecting is refused. The socket is bound and never put
/// to listening. A listener dropped instead stays open in a child another
/// test forks before its exec, and the connection is then accepted.
pub fn stale_socket(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    unsafe {
        let mut address: libc::sockaddr_un = std::mem::zeroed();
        assert!(bytes.len() < address.sun_path.len(), "the socket path fits");
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as libc::c_char;
        }
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        assert!(fd >= 0, "a socket opens");
        let bound = libc::bind(
            fd,
            (&raw const address).cast(),
            std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
        );
        libc::close(fd);
        assert_eq!(bound, 0, "the socket binds");
    }
}
