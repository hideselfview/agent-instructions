use super::*;
use crate::fail::Fail;
use crate::host::tests::fake_ssh;
use crate::host::{LOCAL, hosts_file, record_refusal, refusals};
use crate::host_installed::Installed;
use crate::testing::Scratch;

/// What `SCRIPT` prints on a Mac with every setting on, as m1 printed it.
const ALL_ON: &str = "ui_user=dmitry
ui_automation=Automation Mode is disabled. This device DOES NOT REQUIRE user authentication to enable Automation Mode.
ui_developer_mode=Developer mode is currently enabled.
ui_developer_group=yes dmitry is a member of _developer
";

/// `ALL_ON` with the line of <key> saying <said>.
fn with(key: &str, said: &str) -> String {
    ALL_ON
        .lines()
        .map(|line| match line.split_once('=') {
            Some((at, _)) if at == key => format!("{key}={said}\n"),
            _ => format!("{line}\n"),
        })
        .collect()
}

fn settings(missing: &[Setting]) -> UiTestSettings {
    UiTestSettings {
        user: "dmitry".to_string(),
        missing: missing.to_vec(),
    }
}

fn host(name: &str) -> Host {
    Host {
        band: None,
        name: name.to_string(),
        target: Some(format!("u@{name}")),
        cores: 10,
        speed: Some(1.0),
    }
}

/// A setting is on only when its tool says so; any other answer, a tool that
/// is not there included, counts as off.
#[test]
fn reads_which_settings_a_host_lacks() {
    assert_eq!(parse(ALL_ON).unwrap(), settings(&[]));
    let lacks = |key: &str, said: &str| parse(&with(key, said)).unwrap().missing;
    assert_eq!(
        lacks(
            "ui_automation",
            "Automation Mode is disabled. This device REQUIRES user authentication to enable Automation Mode."
        ),
        [Setting::AutomationMode]
    );
    assert_eq!(
        lacks(
            "ui_automation",
            "sh: /usr/bin/automationmodetool: No such file or directory"
        ),
        [Setting::AutomationMode]
    );
    assert_eq!(
        lacks("ui_developer_mode", "Developer mode is currently disabled."),
        [Setting::DeveloperMode]
    );
    assert_eq!(
        lacks(
            "ui_developer_mode",
            "Developer mode rights are in an unknown state."
        ),
        [Setting::DeveloperMode]
    );
    assert_eq!(
        lacks(
            "ui_developer_group",
            "no dmitry is NOT a member of _developer"
        ),
        [Setting::DeveloperGroup]
    );
    assert!(parse(&with("ui_user", "")).is_err());
    assert!(
        parse(&with("ui_user", "a b")).is_err(),
        "the user goes into a command"
    );
    assert!(parse("ui_user=dmitry\n").is_err());
}

#[test]
fn each_setting_names_the_command_that_turns_it_on() {
    let missing = settings(&Setting::ALL);
    assert_eq!(
        missing.commands(),
        [
            "sudo automationmodetool enable-automationmode-without-authentication",
            "sudo DevToolsSecurity -enable",
            "sudo dseditgroup -o edit -a dmitry -t user _developer",
        ]
    );
}

/// `read` runs the real checks, which only read, through the host's shell
/// (here a fake ssh that runs them on this Mac) and parses what they print.
#[test]
fn reads_a_hosts_settings_over_ssh() {
    let scratch = Scratch::new("ui-settings-read");
    let places = scratch.places();
    let ssh = fake_ssh(&scratch, &scratch.dir.join("remote-home"));
    let found = read(&places, &ssh, &host("m1")).unwrap();
    let me = String::from_utf8(
        std::process::Command::new("id")
            .arg("-un")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(found.user, me.trim());
    let gone = Host {
        target: Some("unreachable".to_string()),
        ..host("gone")
    };
    assert!(read(&places, &ssh, &gone).is_err());
}

/// The places of a scratch with host m1 recorded, and m1 recorded as timing
/// out turning on automation mode.
fn timed_out(scratch: &Scratch) -> Places {
    let places = scratch.places();
    let file = hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 u@m1 10\n").unwrap();
    record_refusal(
        &places,
        Refused::UiAutomation,
        "m1",
        "Timed out while enabling automation mode.",
    )
    .unwrap();
    places
}

fn recorded(places: &Places) -> Vec<(String, Refused)> {
    refusals(places)
        .unwrap()
        .into_iter()
        .map(|refusal| (refusal.host, refusal.refused))
        .collect()
}

/// A host with every setting on needs no sudo, and setup removes its record
/// of timing out, so mac app builds use it again.
#[test]
fn setup_of_a_host_with_every_setting_runs_nothing_and_clears_its_record() {
    let scratch = Scratch::new("ui-settings-on");
    let places = timed_out(&scratch);
    let said = set_up_with(&places, "m1", false, settings(&[]), &mut |_| {
        panic!("nothing to run")
    })
    .unwrap();
    assert_eq!(said, "has the settings that UI tests need");
    assert_eq!(recorded(&places), []);
}

/// With no one at the terminal, setup fails naming the commands to run on
/// the host, runs nothing, and keeps the record.
#[test]
fn setup_with_no_one_at_the_terminal_fails_naming_the_commands() {
    let scratch = Scratch::new("ui-settings-no-terminal");
    let places = timed_out(&scratch);
    let Err(Fail::Message(message)) = set_up_with(
        &places,
        "m1",
        false,
        settings(&[Setting::AutomationMode, Setting::DeveloperGroup]),
        &mut |_| panic!("no one can type the password"),
    ) else {
        panic!("expected a failure");
    };
    assert!(
        message.starts_with("host m1 lacks settings that UI tests need (Automation Mode without a password, dmitry in the _developer group)")
            && message.contains("Run `sudo automationmodetool enable-automationmode-without-authentication`; `sudo dseditgroup -o edit -a dmitry -t user _developer` on host m1, then `builds host setup m1` checks again")
            && !message.contains("DevToolsSecurity"),
        "{message}"
    );
    assert_eq!(
        recorded(&places),
        [("m1".to_string(), Refused::UiAutomation)]
    );
}

/// At a terminal, setup runs the missing settings' commands through sudo in
/// one script and clears the record once they succeed.
#[test]
fn setup_at_a_terminal_runs_the_missing_commands() {
    let scratch = Scratch::new("ui-settings-terminal-on");
    let places = timed_out(&scratch);
    let mut ran = Vec::new();
    let said = set_up_with(
        &places,
        "m1",
        true,
        settings(&[Setting::DeveloperMode]),
        &mut |script| {
            ran.push(script.to_string());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(ran, ["sudo DevToolsSecurity -enable"]);
    assert_eq!(said, "turned on developer mode");
    assert_eq!(recorded(&places), []);
}

/// A failed sudo (a wrong password, say) fails setup, naming what sudo said
/// and the commands, and keeps the record.
#[test]
fn setup_fails_when_sudo_fails() {
    let scratch = Scratch::new("ui-settings-sudo-failed");
    let places = timed_out(&scratch);
    let Err(Fail::Message(message)) = set_up_with(
        &places,
        "m1",
        true,
        settings(&[Setting::AutomationMode, Setting::DeveloperMode]),
        &mut |script| {
            assert_eq!(
                script,
                "sudo automationmodetool enable-automationmode-without-authentication && sudo DevToolsSecurity -enable"
            );
            fail!("sudo on host m1 failed (exit status: 1)")
        },
    ) else {
        panic!("expected a failure");
    };
    assert!(
        message.starts_with("host m1 lacks settings that UI tests need (Automation Mode without a password, developer mode), and sudo on host m1 failed (exit status: 1). Run")
            && message.contains("`sudo DevToolsSecurity -enable`"),
        "{message}"
    );
    assert_eq!(
        recorded(&places),
        [("m1".to_string(), Refused::UiAutomation)]
    );
}

fn installed(name: &str, ui_tests: UiTestSettings) -> HostInstalled {
    HostInstalled {
        host: name.to_string(),
        read: Ok(Installed {
            builds: Some("ab12".to_string()),
            mac: Some(crate::host_installed::MacInstalled { ui_tests }),
            sccache_cap: true,
            sccache: None,
        }),
    }
}

fn refusal(name: &str) -> Refusal {
    Refusal {
        host: name.to_string(),
        refused: Refused::UiAutomation,
        at: 1_800_000_000,
        said: "Timed out while enabling automation mode.".to_string(),
    }
}

/// A host that lacks a setting gets one line that names the commands to run
/// there and `builds host setup`; a host with every setting gets none.
#[test]
fn a_host_that_lacks_a_setting_is_a_finding_that_names_the_commands() {
    assert_eq!(findings(&[], &[installed("m1", settings(&[]))]), []);
    let found = findings(
        &[],
        &[
            installed(LOCAL, settings(&[Setting::DeveloperMode])),
            installed("m1", settings(&[])),
        ],
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].command.as_deref(), Some("builds host setup local"));
    assert!(
        found[0]
            .text
            .starts_with("this Mac lacks developer mode, so XCTest asks for a password")
            && found[0]
                .text
                .contains("Run `sudo DevToolsSecurity -enable` on this Mac"),
        "{found:?}"
    );
}

/// A host whose UI tests timed out gets one line, with what xcodebuild
/// printed and, when builds read its settings, the commands for the ones it
/// lacks; `builds host setup` lets mac app builds use it again.
#[test]
fn a_host_whose_ui_tests_timed_out_is_one_finding_with_its_settings() {
    let other = Refusal {
        refused: Refused::Xcode,
        ..refusal("m1")
    };
    let line = |reads: &[HostInstalled]| {
        let found = findings(&[refusal("m1"), other.clone()], reads);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].command.as_deref(), Some("builds host setup m1"));
        assert!(
            found[0]
                .text
                .starts_with("host m1 cannot run steps that open windows: its UI tests time out turning on automation mode (since ")
                && found[0]
                    .text
                    .contains("the step printed: Timed out while enabling automation mode). Mac app builds skip its app checkouts"),
            "{found:?}"
        );
        found[0].text.clone()
    };
    let lacking = line(&[installed("m1", settings(&[Setting::AutomationMode]))]);
    assert!(
        lacking.contains("Host m1 lacks Automation Mode without a password. To run `sudo automationmodetool enable-automationmode-without-authentication` there through sudo and then let mac app builds use host m1 again, run this from a terminal:"),
        "{lacking}"
    );
    let all_on = line(&[installed("m1", settings(&[]))]);
    assert!(
        all_on.ends_with(
            "Host m1 has the settings that UI tests need now. Let mac app builds use it again with"
        ),
        "{all_on}"
    );
    let unread = line(&[HostInstalled {
        host: "m1".to_string(),
        read: Err("ssh failed".to_string()),
    }]);
    assert!(
        unread.contains(
            "To check the settings that UI tests need on host m1, turn on the ones that are off"
        ),
        "{unread}"
    );
}
