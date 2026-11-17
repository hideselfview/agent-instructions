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
/// never source a shell profile. Relative to $HOME.
pub const SCCACHE_CONFIG: &str = "Library/Application Support/Mozilla.sccache/config";

/// The sccache config file in <home>.
pub fn sccache_config_path(home: &Path) -> PathBuf {
    home.join(SCCACHE_CONFIG)
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

/// Does <text>, an sccache config, hold the cap <cap_kib>?
pub fn sccache_config_text_holds(text: &str, cap_kib: u64) -> bool {
    settings(text) == settings(&sccache_config(cap_kib))
}

/// Does the sccache config hold the cap <cap_kib>?
pub fn sccache_config_holds(home: &Path, cap_kib: u64) -> Result<bool> {
    let file = sccache_config_path(home);
    match std::fs::read_to_string(&file) {
        Ok(text) => Ok(sccache_config_text_holds(&text, cap_kib)),
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

/// Moves the stores to this program's schema and puts this program on PATH
/// with <put>, which returns whether it replaced the installed one. Each
/// store begins to record spans at <now>, the moment of the move. <holder>
/// names the builds process whose use of the stores covers this one
/// (`install_gate::VARIABLE`), if any. Gives <say> each line it has for the
/// person.
fn replace_program(
    places: &crate::places::Places,
    now: i64,
    holder: Option<std::ffi::OsString>,
    put: impl FnOnce() -> Result<bool>,
    say: &mut dyn FnMut(String),
) -> Result<bool> {
    let installing = crate::install_gate::begin(places, say)?;
    if crate::migrate::every_store_has_this_schema(places)? {
        // A command that runs the installed program writes the schema this
        // program reads, so install waits for none.
        say(
            "every store has this builds' schema, so install waits for no builds command"
                .to_string(),
        );
        crate::migrate::migrate_app_files(places)?;
        let replaced = put()?;
        installing.remove_ended_uses()?;
        drop(installing);
        return Ok(replaced);
    }
    // Install waits for every command that uses the stores, so it would wait
    // forever for the builds command that started it.
    if let Some(holder) = holder {
        fail!(
            "builds install is running inside builds process {}, and install would wait forever for that process to end; run install from a shell outside it",
            holder.to_string_lossy()
        );
    }
    // No builds command uses the stores from closing the gates until the new
    // program is on PATH, so none writes a store of another schema.
    let closed = installing.close(say)?;
    crate::migrate::migrate_app_files(places)?;
    crate::migrate::migrate_stores(places, now)?;
    let replaced = put()?;
    drop(closed);
    Ok(replaced)
}

/// `builds install`.
pub fn cmd_install(tool: &Tool, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        fail!("usage: builds install");
    }
    let home = &tool.places.home;
    let replaced = replace_program(
        &tool.places,
        tool.now,
        std::env::var_os(crate::install_gate::VARIABLE),
        || put_on_path(home),
        &mut |line| note(line),
    )?;
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
    let me = std::env::current_exe().context(|| "finding this binary".to_string())?;
    install_on_hosts(&tool.places, &crate::host::Ssh::from_env(), &me)
}

/// Installs <program>, this build, on every recorded host
/// (`host_setup::install_on_host`): copies it there, so each host's jobs run
/// the builds this Mac runs, and has it write the host's sccache config with
/// builds' cap and run its sccache server under launchd, so a change of the
/// cap reaches every host when it lands. The function tries every host
/// first, then fails and names each host it could not install on. `builds
/// health` reports a host that runs a different builds, or whose sccache
/// config does not hold builds' cap, until a later install or `builds host
/// setup` fixes it.
fn install_on_hosts(
    places: &crate::places::Places,
    ssh: &crate::host::Ssh,
    program: &Path,
) -> Result<()> {
    let hosts = crate::host::read_remote(places)?;
    let installed = crate::checkout::parallel_map(&hosts, |host| {
        crate::host_setup::install_on_host(places, ssh, host, program)
    });
    let mut failed = Vec::new();
    for (host, result) in hosts.iter().zip(installed) {
        match result {
            Ok((_, said)) => note(format!(
                "installed this builds on host {}: {}",
                host.name,
                said.trim()
            )),
            Err(error) => failed.push(format!("host {}: {}", host.name, error.text())),
        }
    }
    if !failed.is_empty() {
        fail!(
            "builds install updated builds on this Mac but could not install it on these hosts: {}. `builds health` reports each host whose builds differs from this Mac's or whose sccache config does not hold builds' cap, and `builds host setup <name>` installs it there",
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

    /// What a thread that runs `replace_program` tells the test.
    #[derive(Debug, PartialEq)]
    enum Event {
        /// A line `replace_program` gave its `say`.
        Said(String),
        /// `replace_program` called its `put`.
        Put,
        /// `replace_program` returned.
        Done,
    }

    /// How long a test waits for an event that must come before it fails:
    /// the event, not this time, tells the test that something happened.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// Runs `replace_program` on a thread with no holder, and sends what it
    /// says, when it puts the program on PATH and when it returns.
    fn installing(
        places: &crate::places::Places,
    ) -> (
        std::thread::JoinHandle<()>,
        std::sync::mpsc::Receiver<Event>,
    ) {
        let (events, received) = std::sync::mpsc::channel();
        let places = places.clone();
        let install = std::thread::spawn(move || {
            let put = events.clone();
            replace_program(
                &places,
                0,
                None,
                move || {
                    put.send(Event::Put).unwrap();
                    Ok(true)
                },
                &mut |line| events.send(Event::Said(line)).unwrap(),
            )
            .unwrap();
            events.send(Event::Done).unwrap();
        });
        (install, received)
    }

    fn next(events: &std::sync::mpsc::Receiver<Event>) -> Event {
        events.recv_timeout(PATIENCE).expect("the event comes")
    }

    /// A hook run's use of the stores, held for its life, for process <pid>.
    fn hook_run(places: &crate::places::Places, pid: u32) -> crate::install_gate::Use {
        match crate::install_gate::enter_for_life(
            places,
            pid,
            "checks run",
            "10:00:00",
            &mut |line| panic!("the gate is open: {line}"),
        )
        .unwrap()
        {
            crate::install_gate::Entered::Holds(held) => held,
            crate::install_gate::Entered::RunAgain => panic!("the gate is open"),
        }
    }

    /// When every store already has this program's schema, install moves no
    /// store, so it puts the program on PATH while a hook run holds its use,
    /// and leaves the gate open for the next hook run.
    #[test]
    fn an_install_that_changes_no_schema_waits_for_no_command() {
        let scratch = crate::testing::Scratch::new("install-same-schema");
        let places = scratch.places();
        let store = places.lists.join("builds");
        std::fs::create_dir_all(&store).unwrap();
        drop(crate::db::open(&store.join(crate::db::DATABASE), crate::db::Make::Allowed).unwrap());
        let held = hook_run(&places, 11);
        let uses = places.install_dir().join("commands");
        std::fs::write(
            uses.join("99"),
            "process 99, builds checks run, since 09:00:00\n",
        )
        .unwrap();
        let (install, events) = installing(&places);
        assert_eq!(
            next(&events),
            Event::Said(
                "every store has this builds' schema, so install waits for no builds command"
                    .to_string()
            )
        );
        assert_eq!(next(&events), Event::Put);
        assert_eq!(next(&events), Event::Done);
        install.join().unwrap();
        assert!(
            !uses.join("99").exists(),
            "install removes the file of a use whose process died"
        );
        assert!(uses.join("11").exists(), "the hook run still holds its use");
        drop(hook_run(&places, 12));
        drop(held);
    }

    /// When a store has an older schema, install waits for the hook run's
    /// use before it moves the store and puts the program on PATH.
    #[test]
    fn an_install_that_changes_a_schema_waits_for_the_commands_that_use_the_stores() {
        let scratch = crate::testing::Scratch::new("install-older-schema");
        let places = scratch.places();
        let store = crate::migrate::tests::store_before_pushed(&places.lists.join("bae"));
        let held = hook_run(&places, 11);
        let (install, events) = installing(&places);
        assert_eq!(
            next(&events),
            Event::Said("waiting for process 11, builds checks run, since 10:00:00".to_string())
        );
        drop(held);
        assert_eq!(next(&events), Event::Put);
        assert_eq!(next(&events), Event::Done);
        install.join().unwrap();
        assert!(
            crate::db::open_reader(&store).unwrap().is_some(),
            "the store has today's schema"
        );
    }

    /// With no store that has a schema, the stores do not say which schema
    /// the installed program writes, so install waits for the hook run's use.
    #[test]
    fn an_install_with_no_store_waits_for_the_commands_that_use_the_stores() {
        let scratch = crate::testing::Scratch::new("install-no-store");
        let places = scratch.places();
        std::fs::create_dir_all(places.lists.join("builds")).unwrap();
        let held = hook_run(&places, 11);
        let (install, events) = installing(&places);
        assert_eq!(
            next(&events),
            Event::Said("waiting for process 11, builds checks run, since 10:00:00".to_string())
        );
        drop(held);
        assert_eq!(next(&events), Event::Put);
        assert_eq!(next(&events), Event::Done);
        install.join().unwrap();
    }

    /// Every host that answers gets this build and runs its `host
    /// setup-here`, which writes the host's sccache config, so a change of
    /// builds' cap reaches the host. The install fails with an error that
    /// names each host that does not answer.
    #[test]
    fn install_copies_this_build_to_every_host_sets_it_up_and_names_the_ones_it_missed() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = crate::testing::Scratch::new("install-hosts");
        let places = scratch.places();
        let home = scratch.dir.join("remote-home");
        let ssh = crate::host::tests::fake_ssh(&scratch.dir.join("bin"), &home);
        let hosts = crate::host::hosts_file(&places);
        std::fs::create_dir_all(hosts.parent().unwrap()).unwrap();
        std::fs::write(&hosts, "m1 u@m1 10\ngone unreachable 4\n").unwrap();
        // A builds that writes down each command the host runs it with.
        let program = scratch.dir.join("builds");
        std::fs::write(&program, "#!/bin/sh\necho \"$*\" >> \"$HOME/ran.txt\"\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let Err(crate::fail::Fail::Message(message)) = install_on_hosts(&places, &ssh, &program)
        else {
            panic!("expected a failure for the host that does not answer");
        };
        assert!(message.contains("host gone:"), "{message}");
        assert!(!message.contains("host m1"), "{message}");
        assert_eq!(
            std::fs::read(home.join(".local/bin/builds")).unwrap(),
            std::fs::read(&program).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(home.join("ran.txt")).unwrap(),
            "host setup-here\n"
        );
    }
}
