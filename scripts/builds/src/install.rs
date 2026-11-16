//! `builds install`: puts `builds` on PATH and links `builds-linker` to it
//! (see the `linker` module), creates the shared caches and the links to
//! them, writes the sccache config, runs the sccache server under launchd,
//! points Xcode's default DerivedData at the volume, and copies this builds
//! binary to every recorded host. `builds install` runs from a fresh build,
//! never from the installed binary (that fails: it would copy itself onto
//! itself); it changes only what differs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::checkout::absolute_link_target;
use crate::fail;
use crate::fail::{Context, Result};
use crate::manifest::SHARED_CACHES;
use crate::sccache::{Job, launchd_serves, read_job};
use crate::text::{human, note, tilde};
use crate::tool::Tool;

/// The launchd label of the sccache server builds runs on each Mac.
pub const SCCACHE_LABEL: &str = "local.sccache-server";

/// The sccache config file, which sccache reads on every client and server
/// start, whatever the caller's environment, so the cap holds for builds that
/// never source a shell profile.
pub fn sccache_config_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Mozilla.sccache/config")
}

/// The file the sccache server launchd runs writes its errors to.
fn sccache_log_path(home: &Path) -> PathBuf {
    home.join("Library/Logs/sccache-server.log")
}

/// The config `install` writes for a cap of <cap_kib>.
fn sccache_config(cap_kib: u64) -> String {
    format!(
        "# Written by builds install.\n[cache.disk]\nsize = {}\n",
        cap_kib * 1024
    )
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
        && settings(&existing) == settings(content)
    {
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
    <string>{log}</string>
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
        log = sccache_log_path(home).display()
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
                std::fs::read_link(link)
                    .map(|t| t.display().to_string())
                    .unwrap_or_default(),
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
        let status = Command::new("mv")
            .arg(link)
            .arg(dir)
            .status()
            .context(|| format!("moving {shown}"))?;
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
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| {
                std::fs::metadata(candidate)
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
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
    let staged = bin.join(format!(".builds.{}", crate::text::staged_suffix()));
    std::fs::copy(&me, &staged)
        .context(|| format!("copying {} to {}", me.display(), staged.display()))?;
    std::fs::rename(&staged, &dest).context(|| format!("replacing {}", dest.display()))?;
    note(format!("copied {} to ~/.local/bin/builds", me.display()));
    link_linker(home)?;
    Ok(true)
}

/// Makes ~/.local/bin/builds-linker a symlink to builds, which runs as the
/// linker by that name (`linker`).
fn link_linker(home: &Path) -> Result<()> {
    let link = crate::linker::path(home);
    if std::fs::read_link(&link).is_ok_and(|target| target == Path::new("builds")) {
        return Ok(());
    }
    match std::fs::remove_file(&link) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => fail!("removing {}: {error}", link.display()),
    }
    std::os::unix::fs::symlink("builds", &link)
        .context(|| format!("linking {} to builds", link.display()))?;
    note(format!(
        "linked ~/.local/bin/{} to builds",
        crate::linker::NAME
    ));
    Ok(())
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
            fail!(
                "stopping the dashboard (pid {pid}): {}",
                std::io::Error::last_os_error()
            );
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
        note(format!(
            "restarted the dashboard (was pid {pid}, now pid {})",
            child.id()
        ));
    }
    Ok(())
}

/// Runs `launchctl <args>`, and fails with what launchctl said when it
/// fails.
fn launchctl(args: &[&str]) -> Result<()> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .context(|| "running launchctl".to_string())?;
    if !output.status.success() {
        fail!(
            "launchctl {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&[output.stdout, output.stderr].concat()).trim()
        );
    }
    Ok(())
}

/// builds' cap for the sccache cache, from the shared caches.
pub fn sccache_cap_kib() -> u64 {
    SHARED_CACHES
        .iter()
        .find(|cache| cache.name == "sccache")
        .map(|cache| cache.cap_kib)
        .expect("the shared caches list the sccache cache")
}

/// The last five lines of the file at <path> that hold text, read from its
/// last 16 KiB, or what kept builds from reading it.
fn last_lines(path: &Path) -> String {
    match crate::text::file_end(path, 16 * 1024) {
        Ok((text, _)) => {
            let lines: Vec<&str> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            if lines.is_empty() {
                return "nothing".to_string();
            }
            lines[lines.len().saturating_sub(5)..].join(" | ")
        }
        Err(error) => format!("nothing builds could read ({error})"),
    }
}

/// Why launchd's job does not serve sccache's port, from what launchd says
/// about the job, the pid listening on the port, and the end of the server's
/// log. `confirm_serving` calls `not_serving` only once `launchd_serves`
/// said no, so a pid listening on the port is never the job's.
fn not_serving(job: Option<&Job>, listener: Option<u32>, log: &str, log_end: &str) -> String {
    let launchd = match job {
        Some(job) => format!("launchctl print says {}", job.text()),
        None => "launchd has not loaded the job".to_string(),
    };
    let port = match listener {
        None => format!("no process listens on port {}", crate::sccache::PORT),
        Some(pid) => format!(
            "pid {pid}, which launchd does not run, listens on port {}",
            crate::sccache::PORT
        ),
    };
    format!("{launchd}; {port}; {log} ends with: {log_end}")
}

/// Waits until launchd's job serves sccache's port, asking <look> for the job
/// and the listening pid, and returns the job's pid. Fails after <limit>
/// with what launchd says, which process holds the port, and the end of the
/// server's log at <log>.
fn confirm_serving(
    mut look: impl FnMut() -> Result<(Option<Job>, Option<u32>)>,
    limit: Duration,
    log: &Path,
    shown_log: &str,
) -> Result<u32> {
    let start = Instant::now();
    loop {
        let (job, listener) = look()?;
        if launchd_serves(job.as_ref(), listener) {
            return Ok(listener.expect("a serving job listens"));
        }
        if start.elapsed() >= limit {
            fail!(
                "launchd's sccache server does not serve sccache's port {} seconds after launchctl bootstrap: {}",
                limit.as_secs(),
                not_serving(job.as_ref(), listener, shown_log, &last_lines(log))
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// How long builds waits for the compiles of a server it is about to stop.
const COMPILES_LIMIT: Duration = Duration::from_secs(10 * 60);

/// How long launchd's server may take to listen after launchctl bootstrap.
const START_LIMIT: Duration = Duration::from_secs(10);

/// Writes the sccache config with builds' cap and runs the sccache server
/// under launchd (`SCCACHE_LABEL`) for user <uid>, whose home is <home>.
/// `builds install` calls this on this Mac, and `builds host setup` on each
/// host. When the settings are unchanged and launchd's server is the one
/// listening on sccache's port, the function changes nothing. Otherwise it
/// waits for the compiles of the listening server to finish, stops that
/// server, whether launchd runs it or a client started it, starts launchd's,
/// and checks that launchd's server is the one listening. It fails, with what
/// launchd and the server's log say, when it is not.
pub fn run_sccache_under_launchd(home: &Path, uid: u32) -> Result<()> {
    let sccache_cap = sccache_cap_kib();
    let Some(sccache) = on_path("sccache") else {
        fail!("sccache is not installed")
    };
    let config = sccache_config_path(home);
    let plist_path = home.join(format!("Library/LaunchAgents/{SCCACHE_LABEL}.plist"));
    for file in [&config, &plist_path] {
        let dir = file.parent().expect("the file has a folder");
        std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    }
    let mut changed = false;
    if write_if_changed(&config, &sccache_config(sccache_cap))? {
        note(format!(
            "wrote {} ({} cache)",
            config.display(),
            human(sccache_cap)
        ));
        changed = true;
    }
    if write_if_changed(&plist_path, &plist(&sccache, home))? {
        note(format!("wrote {}", plist_path.display()));
        changed = true;
    }
    let domain = format!("gui/{uid}");
    let service = format!("{domain}/{SCCACHE_LABEL}");
    let job = read_job(&service)?;
    let listener = crate::sccache::listener()?;
    if !changed && launchd_serves(job.as_ref(), listener) {
        note("the sccache server already runs under launchd with these settings");
        return Ok(());
    }
    if let Some(pid) = listener {
        crate::sccache::wait_for_compiles(pid, COMPILES_LIMIT)?;
    }
    if job.is_some() {
        launchctl(&["bootout", &service])?;
    }
    // A server that a client started holds sccache's port, and launchd's
    // server could not listen there.
    if crate::sccache::listening()? {
        crate::sccache::stop(&sccache)?;
    }
    launchctl(&["bootstrap", &domain, &plist_path.to_string_lossy()])?;
    let log = sccache_log_path(home);
    let pid = confirm_serving(
        || Ok((read_job(&service)?, crate::sccache::listener()?)),
        START_LIMIT,
        &log,
        &tilde(&log, home),
    )?;
    note(format!(
        "started the sccache server under launchd (pid {pid}, {})",
        plist_path.display()
    ));
    Ok(())
}

/// `builds install`.
pub fn cmd_install(tool: &Tool, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        fail!("usage: builds install");
    }
    let home = &tool.places.home;
    // Install waits for every command that uses the stores, so it would wait
    // forever for the builds command that started it.
    if let Ok(holder) = std::env::var(crate::install_gate::VARIABLE) {
        fail!(
            "builds install is running inside builds process {holder}, and install would wait forever for that process to end; run install from a shell outside it"
        );
    }
    // No builds command uses the stores from closing the gates until the new
    // program is on PATH, so none writes a store of another schema.
    let closed = crate::install_gate::close(&tool.places, &mut |line| note(line))?;
    crate::migrate::migrate_app_files(&tool.places)?;
    crate::migrate::migrate_stores(&tool.places)?;
    let replaced = put_on_path(home)?;
    drop(closed);
    tool.places.require_volume()?;
    let caches = tool.places.caches();
    let derived_data = tool.places.derived_data();
    for dir in [&caches, &derived_data] {
        std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    }
    for cache in SHARED_CACHES {
        link_cache(&caches.join(cache.name), &cache.link_path(home), home)?;
    }
    run_sccache_under_launchd(home, tool.places.uid)?;
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
            .args([
                "write",
                "com.apple.dt.Xcode",
                "IDECustomDerivedDataLocation",
                "-string",
            ])
            .arg(&derived_data)
            .status()
            .context(|| "running defaults".to_string())?;
        if !status.success() {
            fail!("defaults write com.apple.dt.Xcode IDECustomDerivedDataLocation failed");
        }
        note(format!(
            "pointed Xcode's default DerivedData at {} (was {}); a running Xcode uses it once relaunched",
            derived_data.display(),
            if current.is_empty() {
                "~/Library/Developer/Xcode/DerivedData".to_string()
            } else {
                current
            }
        ));
    }
    if replaced {
        restart_dashboards(home)?;
    }
    install_on_hosts(&tool.places, &crate::host::Ssh::from_env())
}

/// Copies this build to every recorded host (`host_setup::install_program`), so
/// each host's jobs run the builds this Mac runs. The function tries every
/// host first, then fails and names each host it could not install on.
/// `builds health` reports a host that runs a different builds until a later
/// install or `builds host setup` copies the new one there.
fn install_on_hosts(places: &crate::places::Places, ssh: &crate::host::Ssh) -> Result<()> {
    let hosts = crate::host::read_remote(places)?;
    let installed =
        crate::checkout::parallel_map(&hosts, |host| crate::host_setup::install_program(ssh, host));
    let mut failed = Vec::new();
    for (host, result) in hosts.iter().zip(installed) {
        match result {
            Ok(_) => note(format!("installed this builds on host {}", host.name)),
            Err(error) => failed.push(format!("host {}: {}", host.name, error.text())),
        }
    }
    if !failed.is_empty() {
        fail!(
            "builds install updated builds on this Mac but could not copy it to these hosts: {}. `builds health` reports each host whose builds differs from this Mac's, and `builds host setup <name>` installs it there",
            failed.join("; ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sccache::parse_job;
    use crate::sccache::tests::{RESPAWNING, RUNNING};

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
        assert!(
            refusal(&link).contains("install"),
            "a link to the installed binary is the installed binary"
        );
        require_new_build(&other, &installed).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Setup confirms launchd's server once it listens, and when it never
    /// does, fails with what launchd says, which process holds the port, and
    /// the end of the server's log.
    #[test]
    fn confirming_launchds_server_waits_for_it_and_fails_with_what_launchd_and_the_log_say() {
        let scratch = crate::testing::Scratch::new("install-confirm");
        let log = scratch.dir.join("sccache-server.log");
        let mut looks = vec![
            (Some(parse_job(RUNNING).unwrap()), Some(2914)),
            (Some(parse_job(RUNNING).unwrap()), None),
            (None, None),
        ];
        let pid = confirm_serving(
            || Ok(looks.pop().unwrap()),
            Duration::from_secs(10),
            &log,
            "the log",
        )
        .unwrap();
        assert_eq!(pid, 2914);

        let mut said = String::new();
        for line in 0..20 {
            said.push_str(&format!("line {line}\n"));
        }
        said.push_str("sccache: error: Address already in use (os error 48)\n\n");
        std::fs::write(&log, said).unwrap();
        let Err(crate::fail::Fail::Message(message)) = confirm_serving(
            || Ok((Some(parse_job(RESPAWNING).unwrap()), Some(77))),
            Duration::from_millis(300),
            &log,
            "the log",
        ) else {
            panic!("expected a failure");
        };
        assert!(
            message.contains("launchctl print says state = spawn scheduled, last exit code = 2"),
            "{message}"
        );
        assert!(
            message.contains("pid 77, which launchd does not run, listens on port 4226"),
            "{message}"
        );
        assert!(
            message.ends_with(
                "the log ends with: line 16 | line 17 | line 18 | line 19 | sccache: error: Address already in use (os error 48)"
            ),
            "{message}"
        );
        assert!(
            not_serving(None, None, "the log", "nothing")
                .starts_with("launchd has not loaded the job; no process listens on port 4226")
        );
        assert!(
            last_lines(&scratch.dir.join("missing.log")).starts_with("nothing builds could read")
        );
    }

    /// Every host that answers gets this build, and the install fails with an
    /// error that names each host that does not answer.
    #[test]
    fn install_copies_this_build_to_every_host_and_names_the_ones_it_missed() {
        let scratch = crate::testing::Scratch::new("install-hosts");
        let places = scratch.places();
        let home = scratch.dir.join("remote-home");
        let ssh = crate::host::tests::fake_ssh(&scratch.dir.join("bin"), &home);
        let hosts = crate::host::hosts_file(&places);
        std::fs::create_dir_all(hosts.parent().unwrap()).unwrap();
        std::fs::write(&hosts, "m1 u@m1 10\ngone unreachable 4\n").unwrap();
        let Err(crate::fail::Fail::Message(message)) = install_on_hosts(&places, &ssh) else {
            panic!("expected a failure for the host that does not answer");
        };
        assert!(message.contains("host gone:"), "{message}");
        assert!(!message.contains("host m1"), "{message}");
        assert_eq!(
            std::fs::read(home.join(".local/bin/builds")).unwrap(),
            std::fs::read(std::env::current_exe().unwrap()).unwrap()
        );
    }
}
