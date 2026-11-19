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
            cache_home: self.dir.join("cache"),
            private_tmp,
            user_temp: None,
            tmpdir: None,
            uid: unsafe { libc::getuid() },
            lists: self.dir.join("lists"),
        }
    }

    /// The programs builds installs: this test program as builds, and a
    /// file of the words "shots" as builds-shots.
    pub fn programs(&self) -> crate::install::Programs {
        let shots = self.dir.join(crate::shots_program::PROGRAM);
        std::fs::write(&shots, "shots").expect("the scratch folder is writable");
        crate::install::Programs {
            builds: std::env::current_exe().expect("the test program has a path"),
            shots,
        }
    }

    pub fn tool(&self, floor_gib: u64) -> Tool {
        Tool::new(
            self.places(),
            Rules {
                floor_kib: floor_gib * 1024 * 1024,
                floor_margin_kib: 0,
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

/// The pid of a process that has exited.
pub fn gone_pid() -> u32 {
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

pub fn git(dir: &Path, args: &[&str]) {
    git_out(dir, args);
}

/// What `git <args>` prints in <dir>, trimmed; fails the test when git fails.
pub fn git_out(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Writes a host's `builds` into <home>'s ~/.local/bin, where host setup
/// installs it: a shell script that runs the lines <body>.
pub fn host_builds(home: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let bin = home.join(".local/bin");
    std::fs::create_dir_all(&bin).expect("creatable");
    let program = bin.join("builds");
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).expect("writable");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("the fake builds can be made runnable");
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

/// A session's inbox socket at <path> for a test, served by a thread: it takes
/// one connection, reads it to its end and keeps its lines. Never a real
/// session's socket.
pub struct FakeInbox {
    lines: std::thread::JoinHandle<Vec<String>>,
}

impl FakeInbox {
    pub fn listen(path: &Path) -> FakeInbox {
        let listener = std::os::unix::net::UnixListener::bind(path).expect("the socket binds");
        let lines = std::thread::spawn(move || {
            use std::io::BufRead;
            let (stream, _) = listener.accept().expect("a connection comes");
            std::io::BufReader::new(stream)
                .lines()
                .map(|line| line.expect("a line reads"))
                .collect()
        });
        FakeInbox { lines }
    }

    /// The lines the connection sent, once it has closed.
    pub fn received(self) -> Vec<String> {
        self.lines.join().expect("the inbox thread ends")
    }
}

/// A tokens.json whose stylesheet holds one property, `--tokens: <value>;`.
pub fn tokens_json(value: &str) -> String {
    serde_json::json!({
        "name": "test",
        "color": { "themes": [{ "id": "dark" }], "tokens": [] },
        "type": { "families": {}, "groups": [] },
        "spacing": { "tokens": [{ "name": "tokens", "value": value }] },
    })
    .to_string()
}

/// A design system in the worktree at <dir> as the bae repo holds one: its
/// sources (tokens.json, whose stylesheet holds `--tokens: <token>;`, and
/// components/src/bundle.css and bundle.js) and the build-bundle.py that
/// builds the bundles from them, with the bundles ignored by git.
/// design_system writes the bundles too, as a worktree that ran the script
/// has them. A commit of `design` holds the sources and the script only.
pub fn design_system(dir: &Path, token: &str, bundle_css: &str, bundle_js: &str) {
    let system = dir.join("design/system");
    let components = system.join("components");
    std::fs::create_dir_all(components.join("src")).expect("creatable");
    let write = |path: PathBuf, text: &str| std::fs::write(path, text).expect("writable");
    write(
        system.join(".gitignore"),
        "tokens.css\ncomponents/bundle.css\ncomponents/bundle.js\n",
    );
    write(system.join("tokens.json"), &tokens_json(token));
    write(components.join("src/bundle.css"), bundle_css);
    write(components.join("src/bundle.js"), bundle_js);
    write(
        system.join("build-bundle.py"),
        "import pathlib\n\
         components = pathlib.Path(__file__).with_name('components')\n\
         for name in ('bundle.css', 'bundle.js'):\n    \
         (components / name).write_text((components / 'src' / name).read_text())\n",
    );
    write(components.join("bundle.css"), bundle_css);
    write(components.join("bundle.js"), bundle_js);
}

/// The ssh for a test that reaches no host: its store records none, so builds
/// never runs it.
pub fn no_ssh() -> crate::host::Ssh {
    crate::host::Ssh {
        program: "false".to_string(),
    }
}
