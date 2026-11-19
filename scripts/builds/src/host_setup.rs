//! Setting a host up to take jobs (`builds host setup`): putting builds
//! there (a Mac gets this Mac's `builds` and `builds-shots` programs; a Linux
//! host first gets what its builds need and then builds builds from this
//! program's sources, host_linux.rs), running the host's sccache server under
//! its service manager, measuring the speed of its cores against this Mac's,
//! and on a Mac turning on the settings that UI tests need there
//! (ui_test_settings.rs) and turning off Spotlight's indexing of its build
//! volume (spotlight.rs).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::fail;
use crate::fail::{Context, Result};
use crate::host::{self, Host, LOCAL, Ssh, sh_quote};
use crate::host_platform::Os;
use crate::install::Programs;
use crate::places::Places;
use crate::tool::Tool;

/// What `setup_host` did on a host.
pub struct SetUp {
    /// The host's builds root.
    pub root: String,
    /// What `builds host setup-here` said there.
    pub said: String,
    /// The host's core speed against this Mac's, now in the hosts file.
    pub speed: f64,
}

/// Sets <host>, which runs <os>, up to take jobs: on Linux installs what
/// its builds need first (`host_linux::prepare`, whose sudo steps need a
/// person at this terminal, <at_terminal>), installs this program there
/// (`install_on_host`) and records the host's core speed against this Mac's
/// (`core_speed`) in the hosts file, timing the same compile there with
/// `builds host speed-here` and here.
pub fn setup_host(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    os: Os,
    at_terminal: bool,
) -> Result<SetUp> {
    let mut said = Vec::new();
    if os == Os::Linux {
        said = crate::host_linux::prepare(places, ssh, host, at_terminal)?;
    }
    let (root, installed) = install_on_host(places, ssh, host, os, Programs::of_this)?;
    said.push(installed);
    let said = said.join("\n");
    let printed = run_builds_there(places, ssh, host, "host speed-here", "timing a compile")?;
    let there = crate::core_speed::parse_line(&printed)?;
    let here = crate::core_speed::time_compile(&speed_folder())?;
    let speed = crate::core_speed::factor(here, there);
    host::record_speed(places, &host.name, speed)?;
    Ok(SetUp { root, said, speed })
}

/// Where `builds host speed-here` and `setup_host` time their compile.
fn speed_folder() -> PathBuf {
    std::env::temp_dir().join(format!("builds-speed-{}", std::process::id()))
}

/// Runs `~/.local/bin/builds <words>` on <host> with its builds root and
/// PATH, and returns what it printed on stdout and stderr, in order. When it
/// fails, the error names the host and holds all of that. <doing> names the
/// step in errors.
fn run_builds_there(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    words: &str,
    doing: &str,
) -> Result<String> {
    ask_builds_there(places, ssh, host, words, doing)?.map_err(fail::Fail::Message)
}

/// `run_builds_there`, with the host's failure apart from this Mac's: the
/// outer error is this Mac's (ssh would not start), and the inner one is the
/// host's (it did not answer, or builds failed there), whose text names the
/// host and holds what builds printed.
pub fn ask_builds_there(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    words: &str,
    doing: &str,
) -> Result<std::result::Result<String, String>> {
    if host.target.is_none() {
        fail!("{LOCAL} is this Mac, not a host to run builds on over ssh");
    }
    let script = format!(
        "{root}if out=$(\"$HOME/.local/bin/builds\" {words} 2>&1); then printf '%s\\n' \"$out\"; else status=$?; printf '%s\\n' \"$out\" >&2; exit $status; fi",
        root = crate::remote::root_script(host)
    );
    crate::remote::Transport { places, ssh, host }.ask(&script, doing)
}

/// Installs builds on <host>, which runs <os>: the programs <programs>
/// gives, this Mac's, on a Mac (`install_program`), or a build of this program's sources on Linux
/// (`host_linux::install_builds`). Then it has that builds run `builds host
/// setup-here` there, which writes the sccache config with builds' cap and
/// runs the host's sccache server under its service manager, as `builds
/// install` does on this Mac. Returns the host's builds root and what
/// setup-here said. `builds install` runs this for every host, so a change of
/// builds' sccache cap reaches each host when it lands.
pub fn install_on_host(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    os: Os,
    programs: impl FnOnce() -> Result<Programs>,
) -> Result<(String, String)> {
    let root = match os {
        Os::Mac => install_program(ssh, host, &programs()?)?,
        Os::Linux => crate::host_linux::install_builds(ssh, host)?,
    };
    let said = run_builds_there(places, ssh, host, "host setup-here", "setting up")?;
    Ok((root, said))
}

/// Copies <programs>, this Mac's, to `~/.local/bin` on <host>, builds-shots
/// first, and creates the host's builds root, which it returns. Each install
/// writes a program and the links of `links::LINKS` under names of its own and renames
/// them into place, so installs that run at the same time on one host, as two
/// landings' do, each leave whole programs. The repo's own
/// `scripts/build-layout.sh` and `scripts/cargo.sh` find builds there, so a
/// repo's hooks and scripts work on the host as they do here. Both Macs run
/// arm64 macOS, so the same programs run on both.
pub fn install_program(ssh: &Ssh, host: &Host, programs: &Programs) -> Result<String> {
    let shots = crate::shots_program::PROGRAM;
    let read = |program: &Path| {
        std::fs::read(program).context(|| format!("reading {}", program.display()))
    };
    send(
        ssh,
        host,
        shots,
        &read(&programs.shots)?,
        &format!(
            "bin=\"$HOME/.local/bin\" && mkdir -p \"$bin\" && part=$(mktemp \"$bin/{shots}.XXXXXX\") && trap 'rm -f \"$part\"' EXIT && cat > \"$part\" && chmod 755 \"$part\" && mv \"$part\" \"$bin/{shots}\""
        ),
    )?;
    let links: String = crate::links::LINKS
        .iter()
        .map(|link| {
            let name = link.name;
            format!(
                " && link=$(mktemp -u \"$bin/{name}.XXXXXX\") && links=\"$links $link\" && ln -s builds \"$link\" && mv \"$link\" \"$bin/{name}\""
            )
        })
        .collect();
    send(
        ssh,
        host,
        "builds",
        &read(&programs.builds)?,
        &format!(
            "{root}bin=\"$HOME/.local/bin\" && mkdir -p \"$bin\" \"$BUILDS_ROOT\" && part=$(mktemp \"$bin/builds.XXXXXX\") && links= && trap 'rm -f \"$part\" $links' EXIT && cat > \"$part\" && chmod 755 \"$part\" && mv \"$part\" \"$bin/builds\"{links} && echo \"$BUILDS_ROOT\"",
            root = crate::remote::root_script(host)
        ),
    )
}

/// Runs <script> on <host> with <bytes>, which hold <name>, on its stdin,
/// and returns what it printed on stdout, trimmed.
pub fn send(ssh: &Ssh, host: &Host, name: &str, bytes: &[u8], script: &str) -> Result<String> {
    use std::io::Write;
    let Some(target) = &host.target else {
        fail!("{LOCAL} is this Mac and already has builds");
    };
    let mut child = ssh
        .command(
            target,
            &format!("sh -c {}", sh_quote(script)),
            host::Terminal::None,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(|| format!("running {} to {}", ssh.program, host.name))?;
    let mut stdin = child
        .stdin
        .take()
        .expect("the command pipes the child's stdin");
    let sent = stdin.write_all(bytes);
    drop(stdin);
    let output = child
        .wait_with_output()
        .context(|| format!("waiting for ssh to {}", host.name))?;
    if !output.status.success() {
        fail!(
            "installing {name} on host {} ({target}) failed ({}): {}",
            host.name,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    sent.context(|| format!("sending {name} to host {}", host.name))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `builds host setup <name>`: runs `setup_host` on the OS a probe finds
/// there, and on a Mac then turns on the settings that UI tests need
/// (`ui_test_settings::set_up`) and turns off Spotlight's indexing of the
/// build volume (`spotlight::set_up`). A sudo command runs only when a person
/// is at this terminal. For this Mac (`local`), the command only does the
/// sudo steps, because `builds install` sets up everything else.
pub fn cmd_setup(tool: &Tool, ssh: &Ssh, name: &str) -> Result<()> {
    use std::io::IsTerminal;
    let Some(host) = host::all(&tool.places)?
        .into_iter()
        .find(|host| host.name == name)
    else {
        fail!("no host named {name} (`builds host list`)");
    };
    let at_terminal = std::io::stdin().is_terminal();
    let os = match &host.target {
        None => Os::here(),
        Some(target) => match host::probe_target(ssh, target) {
            Ok(probe) => probe.platform.os,
            Err(error) => fail!("host {name} is not reachable: {error}"),
        },
    };
    if host.target.is_some() {
        let set_up = setup_host(&tool.places, ssh, &host, os, at_terminal)?;
        for line in set_up.said.lines() {
            println!("host {name}: {line}");
        }
        println!(
            "installed builds on host {name}, with its builds root in {}; its cores compile at {:.2} of this Mac's speed",
            set_up.root, set_up.speed
        );
    }
    if os == Os::Mac {
        let said = crate::ui_test_settings::set_up(&tool.places, ssh, &host, at_terminal)?;
        println!("host {name}: {said}");
        let said = crate::spotlight::set_up(&tool.places, ssh, &host, at_terminal)?;
        println!("host {name}: {said}");
    }
    Ok(())
}

/// `builds host speed-here`: times the fixed compile on this Mac and prints
/// the line `setup_host` reads.
pub fn cmd_speed_here() -> Result<()> {
    let took = crate::core_speed::time_compile(&speed_folder())?;
    println!("{}", crate::core_speed::line(took));
    Ok(())
}

#[cfg(test)]
#[path = "host_setup_tests.rs"]
mod tests;
