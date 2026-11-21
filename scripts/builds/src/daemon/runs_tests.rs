use super::*;
use crate::testing::Scratch;
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc;
use std::time::Duration;

/// A stand-in for `builds`: logs its arguments, then behaves as the control
/// files beside it say, per lane: `fail-<lane>` ends it with status 3,
/// `stop-<lane>` ends it with status 143 (what build-app exits with after
/// SIGTERM), `hold-<lane>` keeps it running until `go-<lane>` exists.
const STUB: &str = r#"#!/bin/sh
dir=$(dirname "$0")
echo "args: $*"
echo "a line on stderr" >&2
while [ -e "$dir/hold-$3" ] && [ ! -e "$dir/go-$3" ]; do sleep 0.05; done
if [ -e "$dir/fail-$3" ]; then echo boom; exit 3; fi
if [ -e "$dir/stop-$3" ]; then echo "the build stopped on SIGTERM" >&2; exit 143; fi
echo built
"#;

struct Setup {
    scratch: Scratch,
    runs: Arc<Runs>,
    changes: mpsc::Receiver<()>,
}

fn setup(name: &str) -> Setup {
    let scratch = Scratch::new(name);
    let places = scratch.places();
    for file in ["demo/app-checkouts", "demo/app-android-checkouts"] {
        let file = places.root.join(file);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "side-1\n").unwrap();
    }
    let stub = scratch.dir.join("stub.sh");
    std::fs::write(&stub, STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let runs = Runs::start(places, vec![stub.to_string_lossy().into_owned()]).unwrap();
    let (sent, changes) = mpsc::channel();
    runs.listen(Box::new(move || {
        let _ = sent.send(());
    }));
    Setup {
        scratch,
        runs,
        changes,
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.runs.kill_started();
    }
}

impl Setup {
    fn page(&self, lane: &str) -> Option<RunPage> {
        self.runs
            .pages("demo", &|_| Vec::new())
            .into_iter()
            .find(|p| p.lane == lane)
    }

    fn control(&self, name: &str) {
        std::fs::write(self.scratch.dir.join(name), "").unwrap();
    }

    /// Waits for the lane's run to be in <phase>, woken by each change.
    fn until(&self, lane: &str, phase: Phase) -> RunPage {
        self.until_page(|| self.page(lane), phase)
    }

    /// Waits for the shots run of <prefix> on the mac lane to be in
    /// <phase>.
    fn until_shots(&self, prefix: &str, phase: Phase) -> RunPage {
        self.until_page(|| self.runs.shots_page("demo", Lane::Mac, prefix), phase)
    }

    /// Waits for the run <page> gives to be in <phase>, woken by each change.
    fn until_page(&self, page: impl Fn() -> Option<RunPage>, phase: Phase) -> RunPage {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(page) = page()
                && page.phase == phase
            {
                return page;
            }
            let left = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("the run reached its phase in time");
            let _ = self
                .changes
                .recv_timeout(left.min(Duration::from_millis(200)));
        }
    }
}

#[test]
fn a_second_start_in_a_lane_is_refused_and_the_other_lane_starts() {
    let s = setup("runs-busy");
    s.control("hold-mac");
    s.runs
        .start_build("demo", "mac", Some("my-branch"))
        .unwrap();
    let again = s.runs.start_build("demo", "mac", None).unwrap_err();
    assert!(
        matches!(&again, StartError::Busy(m) if m.contains("already queued or running")),
        "{again:?}"
    );
    s.runs.start_build("demo", "android", None).unwrap();
    s.until("android", Phase::Succeeded);
    assert_eq!(
        s.page("mac").unwrap().phase,
        Phase::Running,
        "the refused start changed nothing"
    );
    assert_eq!(
        s.page("mac").unwrap().reference.as_deref(),
        Some("my-branch")
    );

    s.control("go-mac");
    let done = s.until("mac", Phase::Succeeded);
    assert!(
        done.output.contains("args: build-app demo mac my-branch"),
        "{}",
        done.output
    );
    assert!(
        s.page("android")
            .unwrap()
            .output
            .contains("args: build-app demo android\n")
    );
    s.runs
        .start_build("demo", "mac", None)
        .expect("a finished lane starts again");
}

/// A running build's log, which is on the volume that has no file events, is
/// watched: a line it prints reaches the listener while the build runs.
#[test]
fn a_line_a_running_build_prints_reaches_the_listener() {
    let s = setup("runs-log-watch");
    s.control("hold-mac");
    s.runs.start_build("demo", "mac", None).unwrap();
    s.until("mac", Phase::Running);
    let log = s.scratch.places().daemon_dir().join("runs/demo.mac.log");
    while s.changes.try_recv().is_ok() {}
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    writeln!(file, "a later line").unwrap();
    s.changes
        .recv_timeout(Duration::from_secs(30))
        .expect("the write to the log was reported");
    assert!(s.page("mac").unwrap().output.contains("a later line"));
}

/// The commit the shots runs of these tests take; the stub's control
/// files for a shots run are named for it.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn a_shots_run_takes_one_component_s_shots_beside_the_lane_s_build() {
    let s = setup("runs-shots");
    let prefix = "mac.component.checkbox.";
    s.control(&format!("hold-{COMMIT}"));
    s.runs
        .start_shots("demo", Lane::Mac, COMMIT, prefix)
        .unwrap();
    let again = s
        .runs
        .start_shots("demo", Lane::Mac, COMMIT, prefix)
        .unwrap_err();
    assert!(matches!(again, StartError::Busy(_)), "{again:?}");
    assert!(
        s.runs.pages("demo", &|_| Vec::new()).is_empty(),
        "the lanes' Build menus show no shots run"
    );
    s.runs.start_build("demo", "mac", None).unwrap();
    s.until("mac", Phase::Succeeded);
    s.control(&format!("go-{COMMIT}"));
    let done = s.until_shots(prefix, Phase::Succeeded);
    assert_eq!(done.reference.as_deref(), Some(COMMIT));
    assert!(
        done.output
            .contains(&format!("args: app-build demo {COMMIT} --shots {prefix}\n")),
        "{}",
        done.output
    );
}

#[test]
fn a_new_shots_run_in_a_lane_deletes_the_ended_runs_of_other_prefixes() {
    let s = setup("runs-shots-ended");
    let dir = s.scratch.places().daemon_dir().join("runs");
    let (first, second) = ("mac.component.checkbox.", "mac.component.row.");
    s.runs
        .start_shots("demo", Lane::Mac, COMMIT, first)
        .unwrap();
    s.until_shots(first, Phase::Succeeded);
    let record = record_file(&dir, "demo", Lane::Mac, Some(first));
    assert!(record.is_file(), "{}", record.display());
    s.runs
        .start_shots("demo", Lane::Mac, COMMIT, second)
        .unwrap();
    assert!(s.runs.shots_page("demo", Lane::Mac, first).is_none());
    assert!(!record.exists() && !log_file(&dir, "demo", Lane::Mac, Some(first)).exists());
    s.until_shots(second, Phase::Succeeded);
}

#[test]
fn a_run_in_the_lanes_queue_is_queued() {
    let s = setup("runs-queued");
    s.control("hold-mac");
    s.runs.start_build("demo", "mac", None).unwrap();
    let pid = s.page("mac").unwrap().pid;
    let queued = s.runs.pages("demo", &|lane| {
        if lane == Lane::Mac {
            vec![pid]
        } else {
            Vec::new()
        }
    });
    assert_eq!(queued[0].phase, Phase::Queued);
    s.control("go-mac");
    s.until("mac", Phase::Succeeded);
}

#[test]
fn a_failed_build_is_recorded_with_its_output() {
    let s = setup("runs-failed");
    s.control("fail-mac");
    s.runs.start_build("demo", "mac", None).unwrap();
    let failed = s.until("mac", Phase::Failed);
    assert_eq!((failed.exit_code, failed.signal), (Some(3), None));
    assert!(
        failed.output.contains("boom") && failed.output.contains("a line on stderr"),
        "{}",
        failed.output
    );
    assert!(failed.ended_at.is_some());

    // The record on disk says the same, so another daemon reads it so.
    let dir = s.scratch.places().daemon_dir().join("runs");
    let record: Record =
        serde_json::from_slice(&std::fs::read(record_file(&dir, "demo", Lane::Mac, None)).unwrap())
            .unwrap();
    assert_eq!(record.ended.map(|e| e.code), Some(Some(3)));
}

#[test]
fn a_build_a_stop_signal_ended_is_stopped_not_failed() {
    let s = setup("runs-stopped");
    s.control("stop-mac");
    s.runs.start_build("demo", "mac", None).unwrap();
    let stopped = s.until("mac", Phase::Stopped);
    assert_eq!(stopped.exit_code, Some(143));
    assert!(stopped.output.contains("stopped on SIGTERM"));

    // The record on disk says the same, so another daemon reads it so.
    let dir = s.scratch.places().daemon_dir().join("runs");
    let record: Record =
        serde_json::from_slice(&std::fs::read(record_file(&dir, "demo", Lane::Mac, None)).unwrap())
            .unwrap();
    assert_eq!(record.ended.map(|e| e.outcome), Some(RunEnd::Stopped));
}

#[test]
fn a_request_for_something_that_isnt_there_starts_nothing() {
    let s = setup("runs-invalid");
    for (repo, lane, reference) in [
        ("demo", "ios", None),
        ("../demo", "mac", None),
        ("", "mac", None),
        ("other", "mac", None),
        ("demo", "mac", Some("--help")),
    ] {
        let refused = s.runs.start_build(repo, lane, reference).unwrap_err();
        assert!(
            matches!(refused, StartError::Invalid(_)),
            "{repo} {lane} {reference:?}: {refused:?}"
        );
    }
    assert!(s.runs.pages("demo", &|_| Vec::new()).is_empty());
}

fn record(pid: i32) -> Record {
    Record {
        repo: "demo".into(),
        lane: "mac".into(),
        reference: None,
        pid,
        started_at: 100,
        ended: None,
        shots: None,
    }
}

#[test]
fn a_run_a_restart_did_not_see_end_is_detached_while_it_runs_and_lost_after() {
    let scratch = Scratch::new("runs-restart");
    let places = scratch.places();
    std::fs::create_dir_all(places.root.join("demo")).unwrap();
    std::fs::write(places.root.join("demo/app-checkouts"), "side-1\n").unwrap();
    let dir = places.daemon_dir().join("runs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        log_file(&dir, "demo", Lane::Mac, None),
        "before the restart\n",
    )
    .unwrap();

    // A live build-app: a shell with that word among its arguments. bash,
    // not sh: macOS's /bin/sh execs the shell /private/var/select/sh names
    // a moment after it starts, and while it execs, the kernel fails a read
    // of its arguments, so the daemon would see no build-app.
    let mut live = Command::new("/bin/bash")
        .args(["-c", "sleep 30; :", "build-app"])
        .spawn()
        .unwrap();
    write_record(&dir, &record(live.id() as i32), Lane::Mac).unwrap();
    let runs = Runs::start(places.clone(), vec!["/nonexistent".into()]).unwrap();
    let page = &runs.pages("demo", &|_| Vec::new())[0];
    assert_eq!(page.phase, Phase::Detached);
    assert_eq!(page.output, "before the restart\n");
    let refused = runs.start_build("demo", "mac", None).unwrap_err();
    assert!(matches!(refused, StartError::Busy(_)), "{refused:?}");

    live.kill().unwrap();
    live.wait().unwrap();
    assert_eq!(runs.pages("demo", &|_| Vec::new())[0].phase, Phase::Lost);
    // No build is running, so another may start: here it fails only because
    // the program is missing, which it says.
    let started = runs.start_build("demo", "mac", None).unwrap_err();
    assert!(matches!(started, StartError::Failed(_)), "{started:?}");
}

#[test]
fn a_record_that_is_not_one_stops_the_daemon_naming_it() {
    let scratch = Scratch::new("runs-damaged");
    let places = scratch.places();
    let dir = places.daemon_dir().join("runs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("demo.mac.json"), "not json").unwrap();
    let error = Runs::start(places, vec!["x".into()])
        .err()
        .expect("refused")
        .text();
    assert!(
        error.contains("demo.mac.json is not a run record"),
        "{error}"
    );
}

#[test]
fn the_page_gets_the_end_of_a_long_log() {
    let scratch = Scratch::new("runs-tail");
    let log = scratch.dir.join("x.log");
    let lines: String = (0..10_000).map(|n| format!("line {n}\n")).collect();
    std::fs::write(&log, &lines).unwrap();
    let (text, cut) = tail(&log);
    assert!(
        cut && text.ends_with("line 9999\n") && text.starts_with("line "),
        "{}",
        &text[..20]
    );
    assert!(text.len() as u64 <= TAIL);
}
