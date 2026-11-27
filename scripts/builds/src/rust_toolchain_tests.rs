use std::path::{Path, PathBuf};
use std::sync::mpsc;

use super::*;
use crate::host::Host;
use crate::testing::{Scratch, fifo, git, repo};

/// A stand-in rustup that keeps what this machine has in `<state>`: the
/// file `components` and `targets` once a toolchain is installed, and a file
/// per program in `programs`. An install adds what it is asked for, after
/// logging its words. While `<state>/started` exists, an install writes to
/// that FIFO and then waits for a line from the FIFO `<state>/release`.
/// While `<state>/fails` exists, an install fails as rustup's did.
const RUSTUP: &str = r#"state=@STATE@
installed() {
  [ -f "$state/components" ] || { echo "error: toolchain 'T' is not installed" >&2; exit 1; }
}
case "$1 $2" in
"component list") installed; cat "$state/components" ;;
"target list") installed; cat "$state/targets" ;;
"run "*) installed; echo "host: box" ;;
"which "*)
  [ -f "$state/programs/$4" ] || { echo "error: '$4' is not installed for the toolchain 'T'." >&2; exit 1; }
  echo "$state/programs/$4" ;;
"toolchain install")
  echo "$*" >> "$state/log"
  if [ -p "$state/started" ]; then echo started > "$state/started"; read -r line < "$state/release"; fi
  if [ -f "$state/fails" ]; then echo "error: detected conflict: 'bin/cargo'" >&2; exit 1; fi
  mkdir -p "$state/programs"
  have() { grep -qx "$1" "$state/components" 2>/dev/null; }
  add() {
    if ! have "$1-box"; then
      echo "$1-box" >> "$state/components"
      case "$1" in rustc | cargo | rustfmt) : > "$state/programs/$1" ;; clippy) : > "$state/programs/cargo-clippy" ;; esac
    fi
  }
  add rustc; add cargo; add rust-std
  touch "$state/targets"
  shift 3
  while [ $# -gt 0 ]; do
    case "$1" in
    --component) for name in $(echo "$2" | tr , ' '); do add "${name%-preview}"; done; shift ;;
    --target) for name in $(echo "$2" | tr , ' '); do echo "$name" >> "$state/targets"; done; shift ;;
    --profile) shift ;;
    esac
    shift
  done ;;
*) echo "the stand-in rustup knows no $*" >&2; exit 2 ;;
esac"#;

/// A machine with a stand-in rustup.
struct Machine {
    scratch: Scratch,
    state: PathBuf,
}

impl Machine {
    fn new(name: &str) -> Machine {
        let scratch = Scratch::new(name);
        let state = scratch.dir.join("rustup-state");
        std::fs::create_dir_all(&state).unwrap();
        scratch.stand_in(
            "rustup",
            &RUSTUP.replace("@STATE@", &sh_quote(&state.to_string_lossy())),
        );
        Machine { scratch, state }
    }

    fn ensure(&self, toolchain: &Toolchain) -> Result<()> {
        ensure(&self.scratch.places(), &self.scratch.system(), toolchain)
    }

    /// The installs rustup ran, each as its words.
    fn installs(&self) -> Vec<String> {
        match std::fs::read_to_string(self.state.join("log")) {
            Ok(log) => log.lines().map(str::to_string).collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("reading the stand-in's log: {error}"),
        }
    }

    /// Runs two jobs that need <toolchain>, the second starting while the
    /// first one's install runs, and returns how each ended.
    fn two_at_once(&self, toolchain: &Toolchain) -> (Result<()>, Result<()>) {
        let (started, release) = (self.state.join("started"), self.state.join("release"));
        fifo(&started);
        fifo(&release);
        let (places, system) = (self.scratch.places(), self.scratch.system());
        std::thread::scope(|scope| {
            let first = scope.spawn(|| ensure(&places, &system, toolchain));
            std::fs::read_to_string(&started).unwrap();
            let (waits, waiting) = mpsc::channel();
            let second = {
                let (places, system) = (&places, &system);
                scope.spawn(move || {
                    ensure_saying(places, system, toolchain, &move |words| {
                        // The test hears the first wait; later ones find it
                        // listening no longer.
                        let _ = waits.send(words);
                    })
                })
            };
            assert_eq!(
                waiting.recv().unwrap(),
                "builds waits while another builds job installs Rust toolchain 1.100.1"
            );
            // A second install, which must not happen, would wait for no one.
            std::fs::remove_file(&started).unwrap();
            std::fs::write(&release, "go\n").unwrap();
            (first.join().unwrap(), second.join().unwrap())
        })
    }
}

fn pinned() -> Toolchain {
    Toolchain {
        channel: "1.100.1".to_string(),
        profile: None,
        components: vec!["clippy".to_string(), "llvm-tools-preview".to_string()],
        targets: vec!["aarch64-linux-android".to_string()],
    }
}

fn said(result: Result<()>) -> String {
    result.unwrap_err().text()
}

#[test]
fn two_jobs_at_once_install_a_toolchain_once() {
    let machine = Machine::new("toolchain-once");
    let (first, second) = machine.two_at_once(&pinned());
    first.unwrap();
    second.unwrap();
    assert_eq!(
        machine.installs(),
        [
            "toolchain install 1.100.1 --no-self-update --component clippy,llvm-tools-preview --target aarch64-linux-android"
        ]
    );
    machine.ensure(&pinned()).unwrap();
    assert_eq!(
        machine.installs().len(),
        1,
        "a complete toolchain installs nothing"
    );
}

#[test]
fn a_failed_install_fails_the_job_that_waited_for_it_and_a_later_job_installs_again() {
    let machine = Machine::new("toolchain-fails");
    std::fs::write(machine.state.join("fails"), "").unwrap();
    let (first, second) = machine.two_at_once(&pinned());
    let first = said(first);
    assert!(
        first.contains("error: detected conflict: 'bin/cargo'"),
        "{first}"
    );
    assert_eq!(said(second), first);
    assert_eq!(machine.installs().len(), 1);

    std::fs::remove_file(machine.state.join("fails")).unwrap();
    machine.ensure(&pinned()).unwrap();
    assert_eq!(machine.installs().len(), 2);
}

#[test]
fn a_missing_component_or_target_installs_it() {
    let machine = Machine::new("toolchain-adds");
    let mut toolchain = pinned();
    machine.ensure(&toolchain).unwrap();
    toolchain.components.push("rustfmt".to_string());
    machine.ensure(&toolchain).unwrap();
    toolchain.targets.push("wasm32-unknown-unknown".to_string());
    machine.ensure(&toolchain).unwrap();
    assert_eq!(machine.installs().len(), 3);
}

#[test]
fn a_component_listed_without_its_program_fails_and_says_to_uninstall() {
    let machine = Machine::new("toolchain-broken");
    machine.ensure(&pinned()).unwrap();
    std::fs::remove_file(machine.state.join("programs/cargo")).unwrap();
    let error = said(machine.ensure(&pinned()));
    assert!(error.contains("'cargo' is not installed"), "{error}");
    assert!(
        error.contains("`rustup toolchain uninstall 1.100.1`"),
        "{error}"
    );
}

#[test]
fn a_profile_reaches_the_install() {
    let machine = Machine::new("toolchain-profile");
    machine
        .ensure(&Toolchain {
            channel: "1.100.1".to_string(),
            profile: Some("minimal".to_string()),
            components: Vec::new(),
            targets: Vec::new(),
        })
        .unwrap();
    assert_eq!(
        machine.installs(),
        ["toolchain install 1.100.1 --no-self-update --profile minimal"]
    );
}

#[test]
fn a_held_prepare_lock_is_not_waited_for() {
    let scratch = Scratch::new("toolchain-preparing");
    let mut places = scratch.places();
    let lock = places.root.join(".toolchains/1.100.1.prepare.lock");
    let held = preparing(&places, Some(&pinned())).unwrap();
    assert!(crate::flock::is_held(&lock).unwrap());
    let mut command = Command::new("true");
    held.pass_to(&mut command);
    let passed: Vec<_> = command.get_envs().collect();
    assert_eq!(
        passed,
        [(
            std::ffi::OsStr::new(PREPARING_VARIABLE),
            Some(std::ffi::OsStr::new("1.100.1"))
        )]
    );
    // The builds a holder starts takes no lock of its own.
    places.preparing = Some("1.100.1".to_string());
    let inner = preparing(&places, Some(&pinned())).unwrap();
    assert!(inner.lock.is_none());
    drop(held);
    assert!(!crate::flock::is_held(&lock).unwrap());
    assert!(preparing(&places, None).unwrap().lock.is_none());
}

#[test]
fn reads_a_toolchain_file_as_rustup_does() {
    let text = "[toolchain]\nchannel = \"1.100.1\"\ncomponents = [\"clippy\", \"llvm-tools-preview\"]\ntargets = [\"aarch64-linux-android\"]\n";
    assert_eq!(parse("rust-toolchain.toml", text), Ok(Some(pinned())));
    assert_eq!(
        parse("rust-toolchain", "1.95.0\n")
            .unwrap()
            .unwrap()
            .channel,
        "1.95.0"
    );
    assert_eq!(
        parse("rust-toolchain.toml", "[toolchain]\npath = \"/opt/rust\"\n"),
        Ok(None)
    );
    assert!(parse("rust-toolchain.toml", "[toolchain]\nchannel = \"../x\"\n").is_err());
    assert!(
        parse(
            "rust-toolchain.toml",
            "[toolchain]\nchannel = \"stable\"\ncomponents = \"clippy\"\n"
        )
        .is_err()
    );
}

#[test]
fn the_nearest_toolchain_file_in_the_checkout_wins() {
    let scratch = Scratch::new("toolchain-nearest");
    let top = scratch.dir.join("demo");
    let inner = top.join("crates/one");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(scratch.dir.join("rust-toolchain"), "1.1.0\n").unwrap();
    let channel = |dir: &Path| in_checkout(&top, dir).unwrap().map(|found| found.channel);
    assert_eq!(
        channel(&inner),
        None,
        "a file above the checkout is not its pin"
    );
    std::fs::write(
        top.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.2.0\"\n",
    )
    .unwrap();
    assert_eq!(channel(&inner), Some("1.2.0".to_string()));
    std::fs::write(inner.join("rust-toolchain"), "1.3.0\n").unwrap();
    assert_eq!(channel(&inner), Some("1.3.0".to_string()));
    assert_eq!(channel(&top), Some("1.2.0".to_string()));
}

#[test]
fn reads_the_toolchain_a_commit_pins() {
    let scratch = Scratch::new("toolchain-commit");
    let top = scratch.dir.join("demo");
    repo(&top, "target 1G\n");
    assert_eq!(at_commit(&top, "HEAD").unwrap(), None);
    std::fs::write(top.join("rust-toolchain"), "1.100.1\n").unwrap();
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-qm", "pin"]);
    std::fs::write(top.join("rust-toolchain"), "1.0.0\n").unwrap();
    assert_eq!(at_commit(&top, "HEAD").unwrap().unwrap().channel, "1.100.1");
}

#[test]
fn a_host_installs_through_its_own_builds_and_a_lost_connection_is_not_a_failed_install() {
    let scratch = Scratch::new("toolchain-host");
    let places = scratch.places();
    let asked = scratch.dir.join("asked");
    let host = Host {
        band: None,
        name: "m1".to_string(),
        target: Some("user@m1".to_string()),
        cores: 10,
        speed: Some(1.0),
    };
    let on = |body: &str| {
        let program = scratch.stand_in(
            "ssh",
            &format!(
                "echo \"$@\" > {}\n{body}",
                sh_quote(&asked.to_string_lossy())
            ),
        );
        let ssh = crate::testing::ssh_at(&program);
        ensure_on(
            &Transport {
                places: &places,
                ssh: &ssh,
                host: &host,
            },
            &pinned(),
        )
    };
    assert_eq!(on("exit 0").unwrap(), OnHost::Ready);
    // ssh gets the script as one quoted word.
    let script = std::fs::read_to_string(&asked)
        .unwrap()
        .replace("'\\''", "'");
    assert!(
        script.contains("builds\" toolchain '1.100.1' --component 'clippy' --component 'llvm-tools-preview' --target 'aarch64-linux-android'"),
        "{script}"
    );
    assert!(script.contains("export RUSTUP_AUTO_INSTALL=0"), "{script}");
    assert!(matches!(
        on("echo 'connection closed' >&2; exit 255").unwrap(),
        OnHost::Unreachable(said) if said.contains("connection closed")
    ));
    let error = on("echo \"error: detected conflict: 'bin/cargo'\" >&2; exit 1")
        .unwrap_err()
        .text();
    assert!(
        error.contains("host m1") && error.contains("detected conflict"),
        "{error}"
    );
}
