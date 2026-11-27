//! Claims, their notes and the notes' states, `status --json`, and the
//! dashboard's answers, the states it derives among them, on a scratch repo
//! with three side worktrees and a scratch build volume: nothing here touches
//! the real build volume or any real checkout.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

mod common;
use common::pruned_now;

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");
const PATIENCE: Duration = Duration::from_secs(30);

/// A repo whose origin/main has one commit; side-1 on a branch two commits
/// ahead of it, with one changed and one untracked file; side-2 on a branch
/// whose one commit landed on origin/main by cherry-pick, under another hash;
/// side-3 on a branch whose one commit landed the same way but with a changed
/// neighbouring line, so its patch differs and only its author time and
/// subject match. Each checkout has the build root and link `builds setup`
/// would make (setup itself also prunes the real temporary folders, so it
/// isn't run here).
const SETUP: &str = r#"
set -euo pipefail
scratch="$1"
git init -q --bare "$scratch/origin.git"
main="$scratch/demo"
side="$scratch/demo-worktrees/side-1"
side2="$scratch/demo-worktrees/side-2"
side3="$scratch/demo-worktrees/side-3"
git init -q -b main "$main"
git -C "$main" config user.email test@example.com
git -C "$main" config user.name test
echo 'target 1G' > "$main/.build-dirs"
echo target > "$main/.gitignore"
seq 1 10 > "$main/ctx"
git -C "$main" add .build-dirs .gitignore ctx
git -C "$main" commit -q -m 'Start'
git -C "$main" remote add origin "$scratch/origin.git"
git -C "$main" push -q origin main
git -C "$main" fetch -q origin
mkdir -p "$scratch/demo-worktrees"
git -C "$main" worktree add -q -b work "$side" origin/main
git -C "$main" worktree add -q -b landed "$side2" origin/main
git -C "$main" worktree add -q -b landed-moved "$side3" origin/main
for top in "$main" "$side" "$side2" "$side3"; do
    if [ "$top" = "$main" ]; then root="$scratch/volume/demo/main"; else root="$scratch/volume/demo/$(basename "$top")"; fi
    mkdir -p "$root/target"
    printf '%s\n' "$top" > "$root/.checkout"
    ln -s "$root/target" "$top/target"
done
head -c 2097152 /dev/urandom > "$scratch/volume/demo/side-1/target/blob"
echo one > "$side/one" && git -C "$side" add one && git -C "$side" commit -q -m 'Add one'
echo two > "$side/two" && git -C "$side" add two && git -C "$side" commit -q -m 'Add two'
echo changed > "$side/one"
echo new > "$side/untracked"
echo three > "$side2/three" && git -C "$side2" add three && git -C "$side2" commit -q -m 'Add three'
sed -i.bak 5s/5/five/ "$side3/ctx" && rm "$side3/ctx.bak" && git -C "$side3" commit -q -am 'Rename five'
echo other > "$main/other" && git -C "$main" add other && git -C "$main" commit -q -m 'Move main on'
sed -i.bak 7s/7/seven/ "$main/ctx" && rm "$main/ctx.bak" && git -C "$main" commit -q -am 'Rename seven'
git -C "$main" cherry-pick landed landed-moved >/dev/null
git -C "$main" push -q origin main
git -C "$main" fetch -q origin
[ -n "$(git -C "$side2" log --format=%h origin/main..HEAD)" ]
[ "$(git -C "$side3" cherry origin/main HEAD | cut -c1)" = + ]
"#;

struct Demo {
    dir: PathBuf,
    started: Vec<Child>,
}

impl Demo {
    fn new(name: &str) -> Demo {
        let dir = PathBuf::from(format!("/private/tmp/bc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let output = Command::new("bash")
            .args(["-c", SETUP, "bash"])
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "setup: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Demo {
            dir,
            started: Vec::new(),
        }
    }

    fn side(&self, n: u32) -> PathBuf {
        self.dir.join(format!("demo-worktrees/side-{n}"))
    }

    fn main(&self) -> PathBuf {
        self.dir.join("demo")
    }

    fn volume(&self) -> PathBuf {
        self.dir.join("volume")
    }

    fn builds(&self) -> Command {
        let mut command = Command::new(BUILDS);
        command
            .env("BUILDS_ROOT", self.volume())
            .env("HOME", self.dir.join("home"))
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("BUILDS_AGENT");
        command
    }

    /// Runs builds, which must succeed; what it printed.
    fn ok(&self, args: &[&str]) -> String {
        let output = self.builds().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "builds {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Runs builds, which must fail, and checks it said <text>.
    fn refuses(&self, text: &str, args: &[&str]) {
        let output = self.builds().args(args).output().unwrap();
        assert!(
            !output.status.success(),
            "builds {args:?} should have failed"
        );
        let said = String::from_utf8_lossy(&output.stderr).to_string()
            + &String::from_utf8_lossy(&output.stdout);
        assert!(
            said.contains(text),
            "builds {args:?} said {said:?}, not {text:?}"
        );
    }

    /// `builds <args>`, with the paths as arguments.
    fn run(&self, args: &[&str]) {
        self.ok(args);
    }

    /// Makes demo a project with one item, which runs in the claimed
    /// worktree <top> on <branch> and waits on a condition.
    fn wait_on_a_condition(&self, top: &str, branch: &str) {
        let details = self.dir.join("item.md");
        std::fs::write(&details, "Keep the worktree until step 8 lands.\n").unwrap();
        let details = path(&details);
        // `item take` reads the projects' files and the rules here.
        for folder in ["projects", "rules"] {
            let rules_root = self.dir.join("home/.codex/agent-instructions");
            std::fs::create_dir_all(rules_root.join(folder)).unwrap();
        }
        self.run(&["lists", "init", "demo", "--repos", "demo", "--by", "test"]);
        self.run(&["area", "add", "work", "--project", "demo", "--by", "test"]);
        self.run(&[
            "item",
            "add",
            "--title",
            "Keep the worktree until step eight",
            "--area",
            "work",
            "--details-file",
            details,
            "--project",
            "demo",
            "--by",
            "test",
        ]);
        self.run(&[
            "item",
            "take",
            "1",
            "--worktree",
            top,
            "--branch",
            branch,
            "--brief",
            details,
            "--tier",
            "implementation",
            "--project",
            "demo",
            "--by",
            "test",
        ]);
        self.run(&[
            "item",
            "wait",
            "1",
            "--on",
            "condition:step 8 lands",
            "--until",
            "ci",
            "--project",
            "demo",
            "--by",
            "test",
        ]);
    }

    /// Adds the worktree <name>, sets up its build root and makes it the mac
    /// app checkout.
    fn add_app_checkout(&self, name: &str) {
        let top = self.dir.join("demo-worktrees").join(name);
        let added = Command::new("git")
            .arg("-C")
            .arg(self.main())
            .args(["worktree", "add", "-q", "--detach"])
            .arg(&top)
            .status()
            .unwrap();
        assert!(added.success(), "git worktree add {name}");
        let top = path(&top).to_string();
        self.run(&["setup", &top]);
        self.run(&["app-checkout", "mac", &top]);
    }

    fn status_json(&self, folders: bool) -> Value {
        let mut args = vec!["status", "--json"];
        if !folders {
            args.push("--no-folders");
        }
        serde_json::from_str(&self.ok(&args)).unwrap()
    }

    fn claim_file(&self, n: u32) -> PathBuf {
        let git = Command::new("git")
            .arg("-C")
            .arg(self.side(n))
            .args(["rev-parse", "--absolute-git-dir"])
            .output()
            .unwrap();
        PathBuf::from(String::from_utf8(git.stdout).unwrap().trim()).join("builds-claim")
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        for child in &mut self.started {
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "never saw {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One request to the dashboard at <port>: its status code and body.
fn get(port: u16, path: &str, host: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let host = host
        .map(str::to_string)
        .unwrap_or(format!("127.0.0.1:{port}"));
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
    assert!(
        !head.to_lowercase().contains("transfer-encoding: chunked"),
        "a chunked reply: {head}"
    );
    let code = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (code, body.to_string())
}

fn claim_fields(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .unwrap()
        .trim_end_matches('\n')
        .split('\t')
        .map(str::to_string)
        .collect()
}

fn checkout(state: &Value, n: usize) -> &Value {
    &state["repos"][0]["checkouts"][n]
}

fn status_block(status: &str, checkout: &str) -> Vec<String> {
    let lines: Vec<&str> = status.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with(&format!("{checkout} ")))
        .unwrap_or_else(|| panic!("no {checkout} in {status}"));
    lines[at..at + 2].iter().map(|l| l.to_string()).collect()
}

#[test]
fn a_note_needs_a_claim_and_main_is_never_claimed() {
    let demo = Demo::new("noclaim");
    let side = path(&demo.side(1)).to_string();
    let main = path(&demo.main()).to_string();
    demo.refuses("has no claim", &["note", &side, "waiting"]);
    demo.refuses("main checkout", &["claim", &main, "someone"]);
    demo.refuses("has no claim", &["note", &main, "waiting"]);
}

#[test]
fn claims_keep_their_notes_and_a_notes_state_until_a_plain_note_clears_it() {
    let demo = Demo::new("notes");
    let side = path(&demo.side(1)).to_string();
    let file = demo.claim_file(1);

    demo.run(&["claim", &side, "agent A"]);
    let fields = claim_fields(&file);
    assert_eq!(fields[0], "agent A");
    // A new claim waits for its item: the starting state, which `builds item
    // take` clears and which health never releases.
    assert_eq!(
        fields[2..],
        ["claimed; its item is not yet taken", "starting"],
        "claim wrote {fields:?}"
    );
    let since = fields[1].clone();

    demo.run(&["note", &side, "waiting", "on", "Linux", "CI"]);
    assert_eq!(
        claim_fields(&file),
        ["agent A", &since, "waiting on Linux CI"]
    );
    let status = demo.ok(&["status"]);
    let block = status_block(&status, "demo/side-1");
    assert!(
        block[0].ends_with(&format!("[work, claimed by agent A since {since}]")),
        "status doesn't show the claim: {status}"
    );
    assert_eq!(
        block[1], "    note: waiting on Linux CI",
        "status doesn't show the note under its checkout: {status}"
    );

    // Claiming again for the same who renews it and keeps the note; another
    // who is refused.
    demo.run(&["claim", &side, "agent A"]);
    let fields = claim_fields(&file);
    assert_eq!(
        (&fields[0], &fields[2]),
        (&"agent A".to_string(), &"waiting on Linux CI".to_string())
    );
    demo.refuses("claimed by agent A", &["claim", &side, "agent B"]);
    demo.refuses("one line without tabs", &["note", &side, "a\tb"]);
    demo.refuses("one line without tabs", &["claim", &side, "agent\tA"]);

    // A note's state: a fourth field, shown by status, kept by a re-claim and
    // cleared by a note without one.
    let since = claim_fields(&file)[1].clone();
    demo.run(&[
        "note",
        &side,
        "--state",
        "waiting-on-you",
        "which",
        "schema",
        "to",
        "keep",
    ]);
    assert_eq!(
        claim_fields(&file),
        ["agent A", &since, "which schema to keep", "waiting-on-you"]
    );
    let block = status_block(&demo.ok(&["status"]), "demo/side-1");
    assert_eq!(block[1], "    note (waiting on you): which schema to keep");
    let claim = demo.status_json(false)["repos"][0]["checkouts"][1]["claim"].clone();
    assert_eq!(
        claim,
        serde_json::json!({
            "who": "agent A",
            "since": since,
            "note": "which schema to keep",
            "note_state": "waiting-on-you",
        })
    );
    demo.run(&["claim", &side, "agent A"]);
    let fields = claim_fields(&file);
    assert_eq!(
        (&fields[0], &fields[2], &fields[3]),
        (
            &"agent A".to_string(),
            &"which schema to keep".to_string(),
            &"waiting-on-you".to_string()
        ),
        "re-claim dropped the state"
    );
    let since = fields[1].clone();
    demo.refuses(
        "needs the note's text",
        &["note", &side, "--state", "winding-down"],
    );
    demo.refuses(
        "is one of waiting-on-you, winding-down, held, got 'done'",
        &["note", &side, "--state", "done", "finished"],
    );
    demo.refuses(
        "is one of waiting-on-you, winding-down, held, got ''",
        &["note", &side, "--state"],
    );
    assert_eq!(
        claim_fields(&file)[3],
        "waiting-on-you",
        "a refused note changed the claim"
    );
    demo.run(&["note", &side, "waiting", "on", "Linux", "CI"]);
    assert_eq!(
        claim_fields(&file),
        ["agent A", &since, "waiting on Linux CI"],
        "a plain note kept the state"
    );
    assert!(demo.status_json(false)["repos"][0]["checkouts"][1]["claim"]["note_state"].is_null());

    // Clearing the note keeps the claim.
    demo.run(&["note", &side]);
    assert_eq!(claim_fields(&file), ["agent A", &since]);
    // side-1's branch and files hold work origin/main lacks, so release
    // refuses and the claim stays.
    demo.refuses("it holds work origin/main lacks", &["release", &side]);
    assert!(file.exists(), "a refused release took the claim");
}

#[test]
fn status_json_lists_checkouts_and_folders_and_a_folders_last_write_is_its_newest_write_inside() {
    let demo = Demo::new("json");
    let side = path(&demo.side(1)).to_string();
    demo.run(&["claim", &side, "agent A"]);
    demo.run(&["note", &side, "waiting", "on", "Linux", "CI"]);

    let fast = demo.status_json(false);
    assert_eq!(fast["repos"][0]["name"], "demo");
    let names: Vec<&str> = fast["repos"][0]["checkouts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["main", "side-1", "side-2", "side-3"]);
    assert_eq!(checkout(&fast, 1)["claim"]["note"], "waiting on Linux CI");
    assert!(
        checkout(&fast, 1)["folders"].is_null(),
        "--no-folders listed folders"
    );
    assert_eq!(checkout(&fast, 0)["branch"], "main");

    let full = demo.status_json(true);
    assert_eq!(checkout(&full, 1)["folders"][0]["link"], "linked");
    assert!(checkout(&full, 1)["total_kib"].as_u64().unwrap() >= 2048);
    demo.refuses("usage: builds status", &["status", "--sizes"]);

    let target = demo.volume().join("demo/side-1/target");
    let set = |file: &Path, stamp: &str| {
        let status = Command::new("touch")
            .args(["-t", stamp])
            .arg(file)
            .status()
            .unwrap();
        assert!(status.success());
    };
    set(&target, "202001010000");
    set(&target.join("blob"), "202101010000");
    let expected = String::from_utf8(
        Command::new("date")
            .args(["-j", "-f", "%Y%m%d%H%M%S", "20210101000000", "+%s"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        demo.status_json(true)["repos"][0]["checkouts"][1]["folders"][0]["last_write"]
            .as_i64()
            .unwrap()
            .to_string(),
        expected.trim()
    );
}

#[test]
fn sizes_count_each_block_once_in_one_call_for_every_path() {
    let demo = Demo::new("du");
    let usage = demo.dir.join("usage");
    std::fs::create_dir_all(usage.join("clones")).unwrap();
    std::fs::create_dir_all(usage.join("plain")).unwrap();
    let random = |file: PathBuf| {
        let mut bytes = vec![0u8; 4 * 1024 * 1024];
        std::fs::File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut bytes)
            .unwrap();
        std::fs::write(file, bytes).unwrap();
    };
    random(usage.join("clones/a"));
    // An APFS clone: its blocks are shared with the original.
    let cloned = Command::new("cp")
        .arg("-c")
        .arg(usage.join("clones/a"))
        .arg(usage.join("clones/b"))
        .status()
        .unwrap();
    assert!(cloned.success(), "cp -c needs APFS");
    random(usage.join("plain/a"));
    let out = demo.ok(&[
        "du",
        path(&usage.join("clones")),
        path(&usage.join("missing")),
        path(&usage.join("plain")),
    ]);
    let sizes: Vec<u64> = out
        .lines()
        .map(|line| line.trim().parse().unwrap())
        .collect();
    assert_eq!(sizes.len(), 3, "measuring printed {out}");
    assert!(
        (4096..4500).contains(&sizes[0]),
        "a folder with a clone took {} KiB, not the 4096 of its shared blocks",
        sizes[0]
    );
    assert_eq!(sizes[1], 0, "a missing path took {} KiB", sizes[1]);
    assert!(
        (4096..4500).contains(&sizes[2]),
        "a plain 4 MiB file took {} KiB",
        sizes[2]
    );
}

/// Prune keeps a side worktree's build folder whole while an agent claims
/// the worktree, even when the folder is over its cap, and deletes it whole
/// once the agent releases it. The test runs prune with --dry-run, because a
/// real prune also deletes build output in the machine's real temporary
/// folders.
#[test]
fn prune_keeps_a_claimed_folder_over_its_cap_and_deletes_it_once_released() {
    let demo = Demo::new("prune");
    let side3 = demo.side(3);
    let side = path(&side3).to_string();
    std::fs::write(side3.join(".build-dirs"), "target 1M\n").unwrap();
    let profile = demo.volume().join("demo/side-3/target/debug");
    std::fs::create_dir_all(profile.join(".fingerprint")).unwrap();
    std::fs::create_dir_all(profile.join("deps")).unwrap();
    std::fs::write(profile.join("deps/other"), vec![7u8; 2 * 1024 * 1024]).unwrap();
    let listing = |profile: &Path| {
        let out = Command::new("find").arg(profile).output().unwrap();
        let mut lines: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        lines.sort();
        lines
    };
    let before = listing(&profile);
    demo.run(&["claim", &side, "agent C"]);
    let out = demo.builds().args(["prune", "--dry-run"]).output().unwrap();
    let said =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("claimed, kept: ") && said.contains("demo/side-3/target"),
        "a claimed folder over its cap wasn't kept: {said}"
    );
    assert_eq!(listing(&profile), before, "a dry run deleted");
    // Release refuses to leave the changed .build-dirs, so the test deletes
    // the claim file itself, since this test checks prune.
    std::fs::remove_file(demo.claim_file(3)).unwrap();
    let out = demo.builds().args(["prune", "--dry-run"]).output().unwrap();
    let said =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("would delete ")
            && said.contains("demo/side-3/target (")
            && said.contains("> 1M)"),
        "an unclaimed folder over its cap wasn't deleted whole: {said}"
    );
}

/// The daemon follows the scratch volume; status asks it for the sizes, and
/// says the same as when it measured them itself. A second daemon on the same
/// volume is refused. It serves the page, the state the page shows, and the
/// same state as events when anything changes; it refuses a Host it doesn't
/// serve; and it derives each side worktree's state from its claim, the
/// note's state and the commits waiting to land. Once it is gone, status
/// measures again.
#[test]
fn the_daemon_serves_the_state_and_the_events_and_derives_each_worktrees_state() {
    let mut demo = Demo::new("daemon");
    let side = path(&demo.side(1)).to_string();
    let side2 = path(&demo.side(2)).to_string();
    // Its name sorts after the side worktrees'.
    demo.add_app_checkout("xcode-1");
    demo.run(&["claim", &side, "agent A"]);
    demo.run(&["note", &side, "waiting", "on", "Linux", "CI"]);
    let measured = demo.status_json(true);

    let port = free_port();
    // The daemon writes its notes to a file in its folder, not to stderr.
    let log = demo.volume().join(".builds-daemon/log");
    pruned_now(&demo.volume());
    let server = demo
        .builds()
        .args(["dashboard", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let server_pid = server.id() as i32;
    demo.started.push(server);
    until("the daemon to follow the volume", || {
        std::fs::read_to_string(&log).is_ok_and(|text| text.contains("following "))
    });

    let sizes = |state: &Value| -> Vec<(String, u64)> {
        state["repos"][0]["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["name"].as_str().unwrap().to_string(),
                    c["total_kib"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(
        sizes(&demo.status_json(true)),
        sizes(&measured),
        "the daemon's sizes differ"
    );
    let more = demo.volume().join("demo/side-1/target/more");
    std::fs::write(&more, vec![9u8; 1024 * 1024]).unwrap();
    let state_after_write = || {
        demo.status_json(true)["repos"][0]["checkouts"][1]["total_kib"]
            .as_u64()
            .unwrap()
    };
    until("the daemon to see a write", || state_after_write() >= 3072);
    std::fs::remove_file(&more).unwrap();
    demo.refuses(
        "a daemon already runs for",
        &["dashboard", "--port", &free_port().to_string()],
    );

    let (code, page) = get(port, "/", None);
    assert_eq!(
        code,
        200,
        "no page: {}",
        std::fs::read_to_string(&log).unwrap()
    );
    assert!(
        page.contains("<title>Builds</title>"),
        "the page has no title Builds"
    );
    // The first state may come before the checkouts are read.
    let mut state = Value::Null;
    until("the state with git and sizes", || {
        let (code, body) = get(port, "/api/state", None);
        assert_eq!(code, 200, "no state");
        state = serde_json::from_str(&body).unwrap();
        state["repos"]
            .as_array()
            .is_some_and(|repos| repos.len() == 1)
            && !checkout(&state, 1)["git"].is_null()
            && !checkout(&state, 1)["total_kib"].is_null()
    });
    let subjects: Vec<&str> = checkout(&state, 1)["git"]["ahead"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["Add two", "Add one"], "commits waiting to land");
    assert_eq!(checkout(&state, 1)["git"]["uncommitted"], 2);
    let generated = state["generated_at"].as_i64().unwrap();
    let last_write = checkout(&state, 1)["git"]["last_write"].as_i64().unwrap();
    assert!(
        (last_write - generated).abs() < 600,
        "uncommitted files' newest write"
    );
    assert!(
        checkout(&state, 2)["git"]["last_write"].is_null(),
        "a clean worktree has a last write"
    );
    assert_eq!(checkout(&state, 1)["git"]["head"]["subject"], "Add two");
    // Main's working tree is the person's: git is asked for HEAD only.
    let main_git = &checkout(&state, 0)["git"];
    assert!(
        main_git["ahead"].is_null()
            && main_git["uncommitted"].is_null()
            && main_git["last_write"].is_null(),
        "asked git about main's working tree: {main_git}"
    );
    assert_eq!(checkout(&state, 1)["state"], "ready-to-land");
    assert!(checkout(&state, 0)["state"].is_null(), "main got a state");
    assert_eq!(
        checkout(&state, 2)["git"]["ahead"],
        serde_json::json!([]),
        "side-2's cherry-picked commit listed as waiting"
    );
    assert_eq!(
        checkout(&state, 3)["git"]["ahead"],
        serde_json::json!([]),
        "side-3's landed commit with a changed patch listed as waiting"
    );
    assert!(checkout(&state, 1)["total_kib"].as_u64().unwrap() >= 2048);
    assert_eq!(
        get(port, "/api/state", Some("example.com")).0,
        403,
        "served a foreign Host"
    );

    // A note written while the page watches reaches it as an event, along
    // with the load the daemon reads while a page watches.
    let events = Command::new("curl")
        .args(["-sN", &format!("http://127.0.0.1:{port}/events")])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut events = KillOnDrop(events);
    let (sender, receiver) = std::sync::mpsc::channel();
    let stdout = events.0.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut seen = String::new();
    let read_until = |wanted: &str, seen: &mut String| {
        let deadline = Instant::now() + PATIENCE;
        while !seen.contains(wanted) {
            let left = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(left) {
                Ok(line) => {
                    seen.push_str(&line);
                    seen.push('\n');
                }
                Err(_) => panic!("never saw {wanted:?} in the events: {seen}"),
            }
        }
    };
    read_until("event: state", &mut seen);
    demo.run(&["note", &side, "seen", "by", "the", "page"]);
    read_until("\"note\":\"seen by the page\"", &mut seen);
    if !seen.contains("\"load\":{\"now\":[") {
        read_until("\"load\":{\"now\":[", &mut seen);
    }
    drop(events);
    demo.run(&["note", &side, "waiting", "on", "Linux", "CI"]);
    demo.run(&["note", &side]);

    // The state the dashboard derives for each side worktree: <side-1>
    // <side-2>, with side-3 free until health retires it. Main and the app
    // checkout have no card, so no state.
    let free_side_3 = std::cell::Cell::new(" free");
    let states = |side_1: &str, side_2: &str| {
        let wanted = format!("None {side_1} {side_2}{} None", free_side_3.get());
        let deadline = Instant::now() + PATIENCE;
        loop {
            let (_, body) = get(port, "/api/state", None);
            let state: Value = serde_json::from_str(&body).unwrap();
            let got = state["repos"][0]["checkouts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["state"].as_str().unwrap_or("None").to_string())
                .collect::<Vec<_>>()
                .join(" ");
            if got == wanted {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "wanted states {wanted}, last saw {got}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    states("ready-to-land", "free");
    demo.run(&["claim", &side2, "agent B"]);
    states("ready-to-land", "working");
    demo.run(&[
        "note",
        &side2,
        "--state",
        "winding-down",
        "last item, then the handoff",
    ]);
    demo.run(&["note", &side, "--state", "winding-down", "last item"]);
    states("ready-to-land", "winding-down");
    demo.run(&[
        "note",
        &side,
        "--state",
        "waiting-on-you",
        "which schema to keep",
    ]);
    demo.run(&["note", &side2, "--state", "waiting-on-you", "run the reset"]);
    states("waiting-on-you", "waiting-on-you");
    demo.run(&["release", &side2]);
    demo.run(&["claim", &side2, "agent B"]);
    states("waiting-on-you", "working");
    // A held claim shows as held, commits waiting to land or not. A hold
    // needs an item that runs in the worktree and waits on something open.
    demo.wait_on_a_condition(&side, "work");
    demo.run(&["note", &side, "--state", "held", "kept until step 8 lands"]);
    let block = status_block(&demo.ok(&["status"]), "demo/side-1");
    assert_eq!(block[1], "    note (held): kept until step 8 lands");
    // Health, asked right after, reads the claim as it is now.
    let health: Value = serde_json::from_slice(
        &demo
            .builds()
            .args(["health", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let held: Vec<&Value> = health["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["checkout"] == "demo/side-1")
        .map(|w| &w["claim"]["note_state"])
        .collect();
    assert_eq!(held, [&Value::from("held")], "health missed the hold");
    // Health retires the free side-3, so the daemon lists it no more.
    assert!(!demo.side(3).exists(), "health left the free side-3");
    free_side_3.set("");
    states("held", "working");
    // Release refuses side-1, which still runs the item, so the test deletes
    // the claim file, as if something outside builds had deleted it.
    demo.refuses("the worktree still runs #1", &["release", &side]);
    std::fs::remove_file(demo.claim_file(1)).unwrap();
    // The hold went with the claim, and the commits wait to land again.
    states("ready-to-land", "working");

    // Once the daemon is gone, status measures again.
    unsafe { libc::kill(server_pid, libc::SIGTERM) };
    let index = demo
        .started
        .iter()
        .position(|child| child.id() as i32 == server_pid)
        .unwrap();
    let _ = demo.started[index].wait();
    assert!(
        demo.status_json(true)["repos"][0]["checkouts"][1]["total_kib"]
            .as_u64()
            .unwrap()
            >= 2048,
        "status after the daemon stopped"
    );
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
