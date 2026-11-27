use super::*;

/// A record file for one test, gone when the value is.
struct Record(PathBuf);

impl Record {
    fn new(name: &str) -> Record {
        let path = std::env::temp_dir().join(format!(
            "builds-test-awake-{name}-{}",
            crate::job_claim::now_nanos()
        ));
        Record(path)
    }

    fn power(&self) -> Power {
        Power::Record(self.0.clone())
    }

    fn lines(&self) -> Vec<String> {
        match std::fs::read_to_string(&self.0) {
            Ok(text) => text.lines().map(str::to_string).collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("reading {}: {error}", self.0.display()),
        }
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn host(target: Option<&str>) -> Host {
    Host {
        name: "m1".to_string(),
        target: target.map(str::to_string),
        cores: 10,
        speed: None,
        band: None,
    }
}

/// The lines for one assertion of <kind> named <name> that this process took
/// and released.
fn took_and_released(kind: &str, name: &str) -> [String; 2] {
    let line = format!("{kind} {} {name}", std::process::id());
    [format!("take {line}"), format!("release {line}")]
}

#[test]
fn work_keeps_this_mac_from_idle_sleep_until_it_ends() {
    let record = Record::new("work");
    let awake = KeepAwake::while_working(&record.power(), "land side-1\nnow").unwrap();
    let [take, release] =
        took_and_released("PreventUserIdleSystemSleep", "builds: land side-1 now");
    assert_eq!(record.lines(), std::slice::from_ref(&take));
    drop(awake);
    assert_eq!(record.lines(), [take, release]);
}

#[test]
fn a_connection_to_a_host_keeps_this_mac_answering_until_it_ends() {
    let record = Record::new("host");
    let awake = KeepAwake::for_host(&record.power(), &host(Some("u@m1")), "a job of demo").unwrap();
    drop(awake);
    assert_eq!(
        record.lines(),
        took_and_released("NetworkClientActive", "builds: a job of demo on host m1")
    );
}

#[test]
fn work_on_this_mac_opens_no_connection_to_keep_answering() {
    let record = Record::new("local");
    drop(KeepAwake::for_host(&record.power(), &host(None), "a job of demo").unwrap());
    assert_eq!(record.lines(), Vec::<String>::new());
}

#[test]
fn the_commands_that_run_work_keep_this_mac_awake_and_the_rest_do_not() {
    let words = |line: &str| -> Vec<String> { line.split(' ').map(str::to_string).collect() };
    for line in [
        "agent run --role implementing 1",
        "agent resume abc",
        "cargo test",
        "lake build",
        "app-build bae work",
        "build-app bae",
        "lock bae --lane mac true",
        "remote run bae main -- true",
        "remote step m1 /w -- true",
        "review /w --kind code",
        "review run /w",
        "review copy /w",
        "land /w",
        "health",
        "checks run",
        "item take 1 --repo bae --branch b",
        "new",
        "setup",
        "app-checkout mac /w",
        "app-clone mac /a /b",
        "prune",
        "install",
        "host setup-here",
        "shots run",
    ] {
        let words = words(line);
        assert!(does_work(&words[0], &words[1..]), "{line}");
    }
    for line in [
        "agent list",
        "agent start",
        "review wait 3",
        "review settle 4 confirmed",
        "review show 3",
        "judge listen",
        "dashboard",
        "status",
        "item show 1",
        "next",
        "queue demo",
        "checks list",
        "shots findings",
        "ack",
    ] {
        let words = words(line);
        assert!(!does_work(&words[0], &words[1..]), "{line}");
    }
}
