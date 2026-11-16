//! The settings a Mac needs before XCTest can run UI tests there while nobody
//! is at its screen. Without them, XCTest asks for a password on the Mac's
//! screen, or gives up and prints "Timed out while enabling automation mode"
//! (`automationmodetool` then says the device requires user authentication),
//! and the test class runs no tests.
//!
//! Each setting is a fact about one host, this Mac included: `builds host
//! setup` checks it, turns it on through sudo when a person is at the
//! terminal, and checks it again; `builds health` reads it with what the host
//! has installed (host_installed.rs) and lists each host that lacks one; and
//! an app build whose step timed out turning on automation mode records the
//! host as refusing mac app builds (host.rs), which setup removes once the
//! settings are in place.

use std::process::Command;

use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::health::Advice;
use crate::host::{self, Host, LOCAL, Refusal, Refused, Ssh, sh_quote};
use crate::host_installed::HostInstalled;
use crate::places::Places;
use crate::text::{local_minute, note};

/// What XCTest prints, within a line, when it could not turn on automation
/// mode for a UI test because the host asked for a password.
pub const AUTOMATION_TIMED_OUT: &str = "Timed out while enabling automation mode";

/// One setting UI tests need on a host.
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
    /// The user that runs builds there, whom the _developer group needs.
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
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
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
    parse(&transport.capture(SCRIPT, "checking the settings UI tests need")?)
}

/// `host m1`, or `this Mac` for this Mac.
fn place(name: &str) -> String {
    if name == LOCAL {
        "this Mac".to_string()
    } else {
        format!("host {name}")
    }
}

/// Runs <script> on <host> with this terminal as its input and output, so
/// sudo there can ask the person for the password.
fn run_at_terminal(ssh: &Ssh, host: &Host, script: &str) -> Result<()> {
    let mut command = match &host.target {
        Some(target) => ssh.command(
            target,
            &format!("sh -c {}", sh_quote(script)),
            host::Terminal::Person,
        ),
        None => {
            let mut command = Command::new("sh");
            command.arg("-c").arg(script);
            command
        }
    };
    let status = command
        .status()
        .context(|| format!("running sudo on {}", place(&host.name)))?;
    if !status.success() {
        fail!("sudo on {} failed ({status})", place(&host.name));
    }
    Ok(())
}

/// `builds host setup`'s step for these settings: checks them on <host>, and
/// when some are off, runs their commands there through sudo if a person is
/// at this terminal (<at_terminal>) to type the password, then checks again.
/// Once all are on, it removes the host's record of UI tests timing out, so
/// mac app builds use the host again. With no one at the terminal, or with a
/// setting still off, it fails, naming the commands to run on the host.
/// Returns what it says about the host.
pub fn set_up(places: &Places, ssh: &Ssh, host: &Host, at_terminal: bool) -> Result<String> {
    set_up_with(
        places,
        &host.name,
        at_terminal,
        &mut || read(places, ssh, host),
        &mut |script| run_at_terminal(ssh, host, script),
    )
}

/// `set_up`, with <read> asking the host for its settings and <run> running a
/// script there at this terminal.
fn set_up_with(
    places: &Places,
    name: &str,
    at_terminal: bool,
    read: &mut dyn FnMut() -> Result<UiTestSettings>,
    run: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    let place = place(name);
    let before = read()?;
    let said = if before.missing.is_empty() {
        "has the settings UI tests need".to_string()
    } else {
        if !at_terminal {
            fail!(
                "{place} lacks settings UI tests need ({}), and no one is at this terminal to type the sudo password. Run these on {place}: {}. Then `builds host setup {name}` checks them again",
                before.whats(),
                before.quoted_commands()
            );
        }
        let script = before.commands().join(" && ");
        note(format!(
            "turning on {} on {place} through sudo, which asks for {}'s password there: {script}",
            before.whats(),
            before.user
        ));
        let ran = run(&script);
        let after = read()?;
        if !after.missing.is_empty() {
            let why = match ran {
                Err(Fail::Message(message)) => format!(" ({message})"),
                _ => String::new(),
            };
            fail!(
                "{place} still lacks settings UI tests need ({}){why}. Run these on {place}: {}. Then `builds host setup {name}` checks them again",
                after.whats(),
                after.quoted_commands()
            );
        }
        format!("turned on {}", before.whats())
    };
    host::clear_refusal(places, Refused::UiAutomation, name)?;
    Ok(said)
}

/// The "needs action" lines about UI tests: one for each host, this Mac
/// included, that lacks a setting UI tests need (as <reads> say) or that
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
    let place = place(name);
    let setup = format!("builds host setup {name}");
    let lacking = settings.filter(|settings| !settings.missing.is_empty());
    let Some(refusal) = refusal else {
        let settings = lacking.expect("a host with no record is listed for a setting that is off");
        return Advice::run(
            format!(
                "{place} lacks {}, so XCTest asks for a password on its screen or times out turning on automation mode, and its UI tests run nothing. Run {} on {place}, or run this from a terminal, which runs them there through sudo:",
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
            "{} lacks {}. From a terminal, run this: it runs {} there through sudo, then lets mac app builds use {place} again:",
            capitalized(&place),
            settings.whats(),
            settings.quoted_commands()
        ),
        (None, Some(_)) => format!(
            "{} has the settings UI tests need now. Let mac app builds use it again with",
            capitalized(&place)
        ),
        (None, None) => format!(
            "From a terminal, run this: it checks the settings UI tests need on {place}, runs the sudo command for each one that is off, then lets mac app builds use {place} again:"
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
