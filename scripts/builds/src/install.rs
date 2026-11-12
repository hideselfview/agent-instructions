//! `builds install`: puts `builds` on PATH, creates the shared caches and the
//! links to them, writes the sccache config, runs the sccache server under
//! launchd, and points Xcode's default DerivedData at the volume. It runs from
//! a fresh build, never from the installed binary (that fails: it would copy
//! itself onto itself); it changes only what differs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::checkout::absolute_link_target;
use crate::fail;
use crate::fail::{Context, Result};
use crate::manifest::SHARED_CACHES;
use crate::text::{human, note, tilde};
use crate::tool::Tool;

const SCCACHE_LABEL: &str = "local.sccache-server";

/// The sccache config file, which sccache reads on every client and server
/// start, whatever the caller's environment, so the cap holds for builds that
/// never source a shell profile.
pub fn sccache_config_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Mozilla.sccache/config")
}

/// The config `install` writes for a cap of <cap_kib>.
fn sccache_config(cap_kib: u64) -> String {
    format!("# Written by builds install.\n[cache.disk]\nsize = {}\n", cap_kib * 1024)
}

/// The lines of <text> that say something to the program reading it: not
/// blank, not a comment (`#` in TOML, `<!-- -->` in a plist). A comment once
/// named the tool's own path, and moving the tool must not count as a change
/// of settings: rewriting the plist restarts the sccache server, which fails
/// every compile in flight.
fn settings(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("<!--"))
        .collect()
}

/// Does the sccache config hold the cap <cap_kib>?
pub fn sccache_config_holds(home: &Path, cap_kib: u64) -> Result<bool> {
    let file = sccache_config_path(home);
    match std::fs::read_to_string(&file) {
        Ok(text) => Ok(settings(&text) == settings(&sccache_config(cap_kib))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => fail!("reading {}: {error}", file.display()),
    }
}

/// Writes <content> to <file> unless <file> already holds the same settings;
/// returns whether it wrote.
fn write_if_changed(file: &Path, content: &str) -> Result<bool> {
    if let Ok(existing) = std::fs::read_to_string(file)
        && settings(&existing) == settings(content) {
            return Ok(false);
        }
    std::fs::write(file, content).context(|| format!("writing {}", file.display()))?;
    Ok(true)
}

fn plist(sccache: &Path, home: &Path) -> String {
    // One server on sccache's default TCP port, started with the system
    // TMPDIR: a server inherits the TMPDIR of whichever client starts it, and
    // a session-scoped TMPDIR deleted later fails every compile. launchd
    // starts it at login, restarts it if it exits, and it never idles out, so
    // no client has a reason to start one of its own. SCCACHE_START_SERVER=1
    // makes this process the server and SCCACHE_NO_DAEMON=1 keeps it from
    // forking, so it is the process launchd watches. launchd starts a job at
    // 256 open files; the server holds every source file it hashes open, and
    // a crate like aws-sdk-s3 runs out at that, so the job raises its own
    // limit.
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by builds install. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{SCCACHE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>TMPDIR="$(getconf DARWIN_USER_TEMP_DIR)" exec "{sccache}"</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>SCCACHE_START_SERVER</key>
    <string>1</string>
    <key>SCCACHE_NO_DAEMON</key>
    <string>1</string>
    <key>SCCACHE_IDLE_TIMEOUT</key>
    <string>0</string>
    <key>SCCACHE_ERROR_LOG</key>
    <string>{home}/Library/Logs/sccache-server.log</string>
  </dict>
  <key>SoftResourceLimits</key>
  <dict>
    <key>NumberOfFiles</key>
    <integer>65536</integer>
  </dict>
  <key>HardResourceLimits</key>
  <dict>
    <key>NumberOfFiles</key>
    <integer>65536</integer>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#,
        sccache = sccache.display(),
        home = home.display()
    )
}

/// Makes <link> (~/.gradle) a symlink to the shared cache <dir> on the
/// volume. A real directory there, from a tool that ran before the link
/// existed, is moved onto the volume whole, since it may hold the tool's own
/// settings beside the cache.
fn link_cache(dir: &Path, link: &Path, home: &Path) -> Result<()> {
    let shown = tilde(link, home);
    if link.is_symlink() {
        if absolute_link_target(link)? != dir {
            fail!(
                "{shown} points at {}, not {}; move what it holds into {}, remove the link, and run agent-instructions' install.sh again",
                std::fs::read_link(link).map(|t| t.display().to_string()).unwrap_or_default(),
                dir.display(),
                dir.display()
            );
        }
        return std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()));
    }
    if link.exists() {
        if dir.exists() {
            fail!(
                "both {shown} and {} exist; move what you need from {shown} into {}, remove {shown}, and run agent-instructions' install.sh again",
                dir.display(),
                dir.display()
            );
        }
        note(format!("moving {shown} to {}", dir.display()));
        let status = Command::new("mv").arg(link).arg(dir).status().context(|| format!("moving {shown}"))?;
        if !status.success() {
            fail!("moving {shown} to {} failed", dir.display());
        }
    } else {
        std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
        }
    }
    std::os::unix::fs::symlink(dir, link).context(|| format!("linking {shown}"))?;
    note(format!("linked {shown} -> {}", dir.display()));
    Ok(())
}

/// The first executable named <name> on PATH.
fn on_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).map(|dir| dir.join(name)).find(|candidate| {
            std::fs::metadata(candidate).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    })
}

/// Fails when <me> is the installed binary <dest>: copying it onto itself
/// installs nothing, and the caller would think the tool just updated.
fn require_new_build(me: &Path, dest: &Path) -> Result<()> {
    if std::fs::canonicalize(dest).ok() == std::fs::canonicalize(me).ok() {
        fail!(
            "this is the installed binary ({}), so installing it changes nothing; build scripts/builds \
             (cargo build --release in agent-instructions) and run that build's `install`, or run \
             agent-instructions' install.sh, which does both",
            dest.display()
        );
    }
    Ok(())
}

/// Copies this binary to ~/.local/bin/builds in one rename, so no hook ever
/// finds the path missing or half written while it is replaced, and a process
/// running the old file keeps that file: writing over it in place invalidates
/// its code signature and the kernel kills it. Returns whether it replaced the
/// binary (always, now: running from the installed binary fails).
fn put_on_path(home: &Path) -> Result<bool> {
    let bin = home.join(".local/bin");
    std::fs::create_dir_all(&bin).context(|| format!("creating {}", bin.display()))?;
    let me = std::env::current_exe().context(|| "finding this binary".to_string())?;
    let dest = bin.join("builds");
    require_new_build(&me, &dest)?;
    let staged = bin.join(format!(".builds.{}", std::process::id()));
    std::fs::copy(&me, &staged).context(|| format!("copying {} to {}", me.display(), staged.display()))?;
    std::fs::rename(&staged, &dest).context(|| format!("replacing {}", dest.display()))?;
    note(format!("copied {} to ~/.local/bin/builds", me.display()));
    Ok(true)
}

/// The dashboard daemons running now: the pid and the arguments after
/// `builds dashboard`, found by their exact command line.
fn running_dashboards() -> Result<Vec<(u32, Vec<String>)>> {
    let listing = Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
        .context(|| "running ps".to_string())?;
    let listing = String::from_utf8_lossy(&listing.stdout);
    Ok(listing
        .lines()
        .filter_map(|line| {
            let (pid, command) = line.trim_start().split_once(' ')?;
            let mut words = command.split_whitespace();
            (words.next()? == "builds" && words.next()? == "dashboard").then_some(())?;
            Some((pid.parse().ok()?, words.map(String::from).collect()))
        })
        .collect())
}

/// Stops each running dashboard daemon and starts it again, detached, from the
/// binary just installed, so it runs the new code. The daemon holds only
/// state it rebuilds from the volume.
fn restart_dashboards(home: &Path) -> Result<()> {
    for (pid, args) in running_dashboards()? {
        let raw = pid as libc::pid_t;
        // SAFETY: kill(2) on a pid ps just listed; no memory is touched.
        if unsafe { libc::kill(raw, libc::SIGTERM) } != 0 {
            fail!("stopping the dashboard (pid {pid}): {}", std::io::Error::last_os_error());
        }
        let mut waited = 0;
        // SAFETY: signal 0 only checks that the process exists.
        while unsafe { libc::kill(raw, 0) } == 0 {
            if waited >= 100 {
                fail!("the dashboard (pid {pid}) did not stop within 10 s");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            waited += 1;
        }
        let child = {
            use std::os::unix::process::CommandExt;
            Command::new(home.join(".local/bin/builds"))
                .arg0("builds")
                .arg("dashboard")
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .context(|| "starting the dashboard".to_string())?
        };
        note(format!("restarted the dashboard (was pid {pid}, now pid {})", child.id()));
    }
    Ok(())
}

fn launchctl(args: &[&str]) -> Result<bool> {
    Ok(Command::new("launchctl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context(|| "running launchctl".to_string())?
        .success())
}

/// `builds install`.
pub fn cmd_install(tool: &Tool, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        fail!("usage: builds install");
    }
    let home = &tool.places.home;
    let replaced = put_on_path(home)?;
    tool.places.require_volume()?;
    let caches = tool.places.caches();
    let derived_data = tool.places.derived_data();
    for dir in [&caches, &derived_data] {
        std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    }
    for cache in SHARED_CACHES {
        link_cache(&caches.join(cache.name), &cache.link_path(home), home)?;
    }
    let Some(sccache_cap) = SHARED_CACHES.iter().find(|cache| cache.name == "sccache").map(|c| c.cap_kib) else {
        fail!("the shared caches list no sccache cache");
    };
    let Some(sccache) = on_path("sccache") else { fail!("sccache is not installed") };
    let config = sccache_config_path(home);
    let config_dir = config.parent().expect("the config has a folder");
    std::fs::create_dir_all(config_dir).context(|| format!("creating {}", config_dir.display()))?;
    let mut restart = false;
    if write_if_changed(&config, &sccache_config(sccache_cap))? {
        note(format!("wrote {} ({} cache)", config.display(), human(sccache_cap)));
        restart = true;
    }
    let plist_path = home.join(format!("Library/LaunchAgents/{SCCACHE_LABEL}.plist"));
    if write_if_changed(&plist_path, &plist(&sccache, home))? {
        note(format!("wrote {}", plist_path.display()));
        restart = true;
    }
    // Restarting the server fails the compiles of every build running now,
    // so it restarts only when its settings changed or launchd isn't running
    // it.
    let domain = format!("gui/{}", tool.places.uid);
    let service = format!("{domain}/{SCCACHE_LABEL}");
    if restart || !launchctl(&["print", &service])? {
        launchctl(&["bootout", &service])?;
        Command::new(&sccache)
            .arg("--stop-server")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context(|| "stopping the sccache server".to_string())?;
        if !launchctl(&["bootstrap", &domain, &plist_path.to_string_lossy()])? {
            fail!("launchctl bootstrap {domain} {} failed", plist_path.display());
        }
        note(format!("started the sccache server under launchd ({})", plist_path.display()));
    } else {
        note("the sccache server already runs under launchd with these settings");
    }
    // xcodebuild runs without -derivedDataPath, and the Xcode app, write
    // here.
    let current = Command::new("defaults")
        .args(["read", "com.apple.dt.Xcode", "IDECustomDerivedDataLocation"])
        .stderr(Stdio::null())
        .output()
        .context(|| "running defaults".to_string())?;
    let current = String::from_utf8_lossy(&current.stdout).trim().to_string();
    if current != derived_data.to_string_lossy() {
        let status = Command::new("defaults")
            .args(["write", "com.apple.dt.Xcode", "IDECustomDerivedDataLocation", "-string"])
            .arg(&derived_data)
            .status()
            .context(|| "running defaults".to_string())?;
        if !status.success() {
            fail!("defaults write com.apple.dt.Xcode IDECustomDerivedDataLocation failed");
        }
        note(format!(
            "pointed Xcode's default DerivedData at {} (was {}); a running Xcode uses it once relaunched",
            derived_data.display(),
            if current.is_empty() { "~/Library/Developer/Xcode/DerivedData".to_string() } else { current }
        ));
    }
    if replaced {
        restart_dashboards(home)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comment_is_no_setting() {
        let old = "# Written by builds install (/Users/x/dev/agent-instructions/scripts/builds/builds).\n[cache.disk]\nsize = 32212254720\n";
        assert_eq!(settings(old), settings(&sccache_config(30 * 1024 * 1024)));
        assert_ne!(settings(old), settings(&sccache_config(20 * 1024 * 1024)));
    }

    #[test]
    fn installing_from_the_installed_binary_fails_loudly() {
        let dir = std::env::temp_dir().join(format!("builds-install-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let installed = dir.join("builds");
        std::fs::write(&installed, "x").unwrap();
        let other = dir.join("fresh");
        std::fs::write(&other, "y").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&installed, &link).unwrap();
        let refusal = |me: &Path| match require_new_build(me, &installed) {
            Err(crate::fail::Fail::Message(message)) => message,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(refusal(&installed).contains("scripts/builds"));
        assert!(refusal(&link).contains("install"), "a link to the installed binary is the installed binary");
        require_new_build(&other, &installed).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
