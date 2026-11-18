//! The settings a Mac needs before XCTest can run UI tests there while nobody
//! is at its screen. Without them, XCTest asks for a password on the Mac's
//! screen, or gives up and prints "Timed out while enabling automation mode"
//! (`automationmodetool` then says the device requires user authentication),
//! and the test class runs no tests.
//!
//! Each setting is a fact about one host, this Mac included: `builds host
//! setup` checks it and turns it on through sudo when a person is at the
//! terminal; `builds health` reads it with what the host
//! has installed (host_installed.rs) and lists each host that lacks one; and
//! an app build whose step timed out turning on automation mode records the
//! host as refusing mac app builds (host.rs), which setup removes once the
//! settings are in place.

use crate::fail;
use crate::fail::Result;
use crate::health::Advice;
use crate::host::{self, Host, Refusal, Refused, Ssh};
use crate::host_installed::HostInstalled;
use crate::places::Places;
use crate::text::local_minute;

/// What XCTest prints, within a line, when it could not turn on automation
/// mode for a UI test because the host asked for a password.
pub const AUTOMATION_TIMED_OUT: &str = "Timed out while enabling automation mode";

/// One setting that UI tests need on a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// Automation Mode turns on without asking for a password.
    AutomationMode,
    /// Developer mode (`DevToolsSecurity`) is on, so a member of _developer
    /// may debug and control other processes without a password.
    DeveloperMode,
    /// The user that runs builds there is a member of the _developer group.
    DeveloperGroup,
}

impl Setting {
    const ALL: [Setting; 3] = [
        Setting::AutomationMode,
        Setting::DeveloperMode,
        Setting::DeveloperGroup,
    ];

    /// The setting in words, for <user>.
    fn what(self, user: &str) -> String {
        match self {
            Setting::AutomationMode => "Automation Mode without a password".to_string(),
            Setting::DeveloperMode => "developer mode".to_string(),
            Setting::DeveloperGroup => format!("{user} in the _developer group"),
        }
    }

    /// The command that turns the setting on for <user>.
    pub fn command(self, user: &str) -> String {
        match self {
            Setting::AutomationMode => {
                "sudo automationmodetool enable-automationmode-without-authentication".to_string()
            }
            Setting::DeveloperMode => "sudo DevToolsSecurity -enable".to_string(),
            Setting::DeveloperGroup => {
                format!("sudo dseditgroup -o edit -a {user} -t user _developer")
            }
        }
    }

    /// The key of the line `SCRIPT` prints for the setting.
    fn key(self) -> &'static str {
        match self {
            Setting::AutomationMode => "ui_automation",
            Setting::DeveloperMode => "ui_developer_mode",
            Setting::DeveloperGroup => "ui_developer_group",
        }
    }

    /// Whether <said>, the line `SCRIPT` printed for the setting, says the
    /// setting is on. Anything else, a tool that is not there included,
    /// counts as off.
    fn on_in(self, said: &str) -> bool {
        match self {
            Setting::AutomationMode => said.contains("DOES NOT REQUIRE user authentication"),
            Setting::DeveloperMode => said.contains("enabled") && !said.contains("disabled"),
            Setting::DeveloperGroup => said.starts_with("yes "),
        }
    }
}

/// What a host said about its settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiTestSettings {
    /// The user that runs builds there, which setup adds to the _developer
    /// group.
    pub user: String,
    /// The settings that are off, in `Setting::ALL`'s order.
    pub missing: Vec<Setting>,
}

impl UiTestSettings {
    /// The settings that are off, in words.
    fn whats(&self) -> String {
        self.missing
            .iter()
            .map(|setting| setting.what(&self.user))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The commands that turn on the settings that are off.
    fn commands(&self) -> Vec<String> {
        self.missing
            .iter()
            .map(|setting| setting.command(&self.user))
            .collect()
    }

    /// The commands that turn on the settings that are off, each in
    /// backticks, for a person to read.
    fn quoted_commands(&self) -> String {
        self.commands()
            .iter()
            .map(|command| format!("`{command}`"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Runs on a host, and on this Mac, to print what `parse` reads: the user,
/// then one line per setting that holds all its tool printed. Each tool only
/// reads.
pub const SCRIPT: &str = r#"echo "ui_user=$(id -un)"
echo "ui_automation=$(/usr/bin/automationmodetool 2>&1 | tr '\n' ' ')"
echo "ui_developer_mode=$(/usr/sbin/DevToolsSecurity -status 2>&1 | tr '\n' ' ')"
echo "ui_developer_group=$(/usr/sbin/dseditgroup -o checkmember -m "$(id -un)" _developer 2>&1 | tr '\n' ' ')""#;

/// Reads what `SCRIPT` printed, among other lines.
pub fn parse(text: &str) -> Result<UiTestSettings> {
    let value = |key: &str| crate::remote::answer_value(text, key);
    let user = match value("ui_user") {
        Some(user)
            if !user.is_empty()
                && user
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) =>
        {
            user.to_string()
        }
        _ => fail!("the host's answer has no user name builds can use: {text:?}"),
    };
    let mut missing = Vec::new();
    for setting in Setting::ALL {
        let Some(said) = value(setting.key()) else {
            fail!("the host's answer has no {} line: {text:?}", setting.key());
        };
        if !setting.on_in(said) {
            missing.push(setting);
        }
    }
    Ok(UiTestSettings { user, missing })
}

/// Asks <host> for its settings.
pub fn read(places: &Places, ssh: &Ssh, host: &Host) -> Result<UiTestSettings> {
    let transport = crate::remote::Transport { places, ssh, host };
    parse(&transport.capture(SCRIPT, "checking the settings that UI tests need")?)
}

/// `builds host setup`'s step for these settings: checks them on <host>, and
/// when some are off, turns them on there through sudo (`host::sudo_step`),
/// which fails with no one at this terminal to type the password. Once all
/// are on, it removes the host's record of UI tests timing out, so mac app
/// builds use the host again. Returns what it says about the host.
pub fn set_up(places: &Places, ssh: &Ssh, host: &Host, at_terminal: bool) -> Result<String> {
    set_up_with(
        places,
        &host.name,
        at_terminal,
        read(places, ssh, host)?,
        &mut |script| host::run_at_terminal(ssh, host, script),
    )
}

/// `set_up`, with <settings> as the host said them and <run> running a
/// script there at this terminal.
fn set_up_with(
    places: &Places,
    name: &str,
    at_terminal: bool,
    settings: UiTestSettings,
    run: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    let said = if settings.missing.is_empty() {
        "has the settings that UI tests need".to_string()
    } else {
        host::sudo_step(
            name,
            at_terminal,
            &format!(
                "{} lacks settings that UI tests need ({})",
                host::place(name),
                settings.whats()
            ),
            &settings.commands(),
            run,
        )?;
        format!("turned on {}", settings.whats())
    };
    host::clear_refusal(places, Refused::UiAutomation, name)?;
    Ok(said)
}

/// The "needs action" lines about UI tests: one for each host, this Mac
/// included, that lacks a setting that UI tests need (as <reads> say) or that
/// builds recorded as timing out turning on automation mode (<refusals>,
/// which may hold other kinds). Each names the commands to run on the host
/// when <reads> know them, and `builds host setup`, which runs them through
/// sudo and lets mac app builds use the host again.
pub fn findings(refusals: &[Refusal], reads: &[HostInstalled]) -> Vec<Advice> {
    let mut names: Vec<&str> = refusals
        .iter()
        .filter(|refusal| refusal.refused == Refused::UiAutomation)
        .map(|refusal| refusal.host.as_str())
        .chain(reads.iter().filter_map(|read| {
            let settings = &read.read.as_ref().ok()?.ui_tests;
            (!settings.missing.is_empty()).then_some(read.host.as_str())
        }))
        .collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let refusal = refusals
                .iter()
                .find(|refusal| refusal.refused == Refused::UiAutomation && refusal.host == name);
            let settings = reads
                .iter()
                .find(|read| read.host == name)
                .and_then(|read| read.read.as_ref().ok())
                .map(|installed| &installed.ui_tests);
            finding(name, refusal, settings)
        })
        .collect()
}

/// The line for host <name>, with its record of timing out when it has one
/// and its settings when builds could read them.
pub fn finding(name: &str, refusal: Option<&Refusal>, settings: Option<&UiTestSettings>) -> Advice {
    let place = host::place(name);
    let setup = format!("builds host setup {name}");
    let lacking = settings.filter(|settings| !settings.missing.is_empty());
    let Some(refusal) = refusal else {
        let settings =
            lacking.expect("findings lists a host with no record only when a setting there is off");
        return Advice::run(
            format!(
                "{place} lacks {}, so XCTest asks for a password on its screen or times out turning on automation mode, and its UI tests run nothing. Run {} on {place}, or, to run them there through sudo, run this from a terminal:",
                settings.whats(),
                settings.quoted_commands()
            ),
            setup,
        );
    };
    let timed_out = format!(
        "UI tests on {place} timed out turning on automation mode (since {}), so mac app builds skip its app checkouts. The step printed: {}.",
        local_minute(refusal.at),
        refusal.said.trim_end_matches('.')
    );
    let next = match (lacking, settings) {
        (Some(settings), _) => format!(
            "{} lacks {}. To run {} there through sudo and then let mac app builds use {place} again, run this from a terminal:",
            capitalized(&place),
            settings.whats(),
            settings.quoted_commands()
        ),
        (None, Some(_)) => format!(
            "{} has the settings that UI tests need now. Let mac app builds use it again with",
            capitalized(&place)
        ),
        (None, None) => format!(
            "To check the settings that UI tests need on {place}, turn on the ones that are off through sudo and then let mac app builds use {place} again, run this from a terminal:"
        ),
    };
    Advice::run(format!("{timed_out} {next}"), setup)
}

/// <text> with its first letter a capital, to start a sentence.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
#[path = "ui_test_settings_tests.rs"]
mod tests;
