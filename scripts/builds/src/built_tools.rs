//! Tools a repo builds from its own sources for its toolchain, such as
//! coven's typed lints' driver, which builds keeps once per key on each Mac
//! and links into every checkout that names that key.
//!
//! A `.build-dirs` line `tool <name> <program> <link>` declares one. The
//! repo's program answers two requests, run in the checkout's top folder:
//!
//! - `<program> key` prints the checkout's key as two words: the toolchain's
//!   id (the commit its rustc was built from) and a hash of the tool's
//!   sources. Two checkouts, of one repo or of two, with the same sources and
//!   toolchain print the same key and share one build.
//! - `<program> build <folder>` builds the tool into <folder>, which it
//!   creates, and writes `<folder>/check`, a program that makes the build
//!   ready to run on the Mac it is on and fails unless it runs there.
//!   `BUILDS_TOOL_WORK` names a folder, one per toolchain on each Mac, where
//!   the program keeps what the next build can reuse (fetched sources, a
//!   build folder), so a new key rebuilds only what its sources changed.
//!
//! Each key's build is the folder `caches/tools/<name>/<toolchain>/<sources>`
//! on the build volume, and `<link>` in the checkout points at it. builds puts
//! a key in place (`ensure`) by copying it from another Mac that has it,
//! else by building it, holding the toolchain folder's lock so that one build
//! at a time uses its work folder. A copied or built folder takes its place
//! only once its `check` passes on this Mac, so a key's folder in place
//! always runs here. Before builds runs a `builds cargo` job on a host, it
//! copies each key the checkout names that this Mac has and the host lacks
//! (`send_to`).
//!
//! `prune_tools` deletes each key's folder and each toolchain's folder that
//! no live checkout's key names.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::checkout::{Root, parallel_map};
use crate::evict::lock_exclusive_creating;
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::host::{self, Host, Ssh, sh_quote};
use crate::manifest::{BuiltTool, built_tools};
use crate::places::Places;
use crate::prune::{Run, prune_listed};
use crate::remote::{Transport, root_script};
use crate::text::note;

/// The variable that names the toolchain's work folder to `<program> build`.
pub const WORK_VARIABLE: &str = "BUILDS_TOOL_WORK";

/// The folder in a toolchain's folder that `BUILDS_TOOL_WORK` names.
const WORK: &str = ".work";

/// The lock a build or a copy of a toolchain's key holds.
const LOCK: &str = ".lock";

/// Is <name> a name builds can use as a folder: letters, digits, `.`, `_`
/// and `-`, not starting with a dot?
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A checkout's key for a tool: the toolchain's id and the sources' hash.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub toolchain: String,
    pub sources: String,
}

/// Reads what `<program> key` printed: two plain names on one line.
pub fn parse_key(text: &str) -> std::result::Result<Key, String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        [toolchain, sources] if is_plain_name(toolchain) && is_plain_name(sources) => Ok(Key {
            toolchain: toolchain.to_string(),
            sources: sources.to_string(),
        }),
        _ => Err(format!(
            "printed {text:?}, not a toolchain and a sources hash"
        )),
    }
}

/// Runs `<program> key` in <top>.
pub fn key(top: &Path, tool: &BuiltTool) -> Result<Key> {
    let program = top.join(&tool.program);
    let output = Command::new(&program)
        .arg("key")
        .current_dir(top)
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running {} key", program.display()))?;
    if !output.status.success() {
        fail!(
            "{} key failed ({}): {}",
            program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_key(&String::from_utf8_lossy(&output.stdout))
        .map_err(|error| Fail::Message(format!("{} key {error}", program.display())))
}

/// `caches/tools/<name>` on <places>' volume.
fn tool_dir(places: &Places, name: &str) -> PathBuf {
    places.caches().join("tools").join(name)
}

/// Where this Mac keeps <key>'s build of tool <name>.
pub fn key_dir(places: &Places, name: &str, key: &Key) -> PathBuf {
    tool_dir(places, name)
        .join(&key.toolchain)
        .join(&key.sources)
}

/// The same folder on <host>, as a shell word that the host's shell expands
/// after `root_script`.
fn key_dir_on(name: &str, key: &Key) -> String {
    format!(
        "\"$BUILDS_ROOT\"/caches/tools/{}/{}/{}",
        sh_quote(name),
        sh_quote(&key.toolchain),
        sh_quote(&key.sources)
    )
}

/// `builds tool <name> [<checkout>]`: puts the checkout's key of tool <name>
/// in place on this Mac, links the checkout to it and prints its folder.
pub fn cmd_tool(places: &Places, ssh: &Ssh, args: &[String]) -> Result<()> {
    let (name, at) = match args {
        [name] => (name, "."),
        [name, at] => (name, at.as_str()),
        _ => fail!("usage: builds tool <name> [<checkout>]"),
    };
    let checkout = crate::checkout::Checkout::find(at)?;
    let tools = built_tools(&checkout.top)?;
    let Some(line) = tools.iter().find(|line| &line.name == name) else {
        fail!(
            "{} declares no tool {name} in its {}",
            checkout.top.display(),
            crate::manifest::MANIFEST
        );
    };
    println!("{}", ensure(places, ssh, &checkout.top, line)?.display());
    Ok(())
}

/// Puts each tool <top>'s manifest declares in place and links it (setup).
pub fn ensure_all(places: &Places, ssh: &Ssh, top: &Path) -> Result<()> {
    for line in built_tools(top)? {
        ensure(places, ssh, top, &line)?;
    }
    Ok(())
}

/// Puts the key of tool <line> that <top> names in place on this Mac, from
/// another Mac or by building it, and points the checkout's link at it.
/// Returns the key's folder.
pub fn ensure(places: &Places, ssh: &Ssh, top: &Path, line: &BuiltTool) -> Result<PathBuf> {
    let key = key(top, line)?;
    let dest = key_dir(places, &line.name, &key);
    if !dest.is_dir() {
        obtain(places, ssh, top, line, &key, &dest)?;
    }
    point(&top.join(&line.link), &dest)?;
    Ok(dest)
}

/// Copies <key> from a host that has it, else builds it, holding the
/// toolchain folder's lock.
fn obtain(
    places: &Places,
    ssh: &Ssh,
    top: &Path,
    line: &BuiltTool,
    key: &Key,
    dest: &Path,
) -> Result<()> {
    let toolchain = dest
        .parent()
        .expect("a key's folder has a toolchain folder");
    std::fs::create_dir_all(toolchain).context(|| format!("creating {}", toolchain.display()))?;
    let _one_at_a_time = lock_exclusive_creating(&toolchain.join(LOCK))?;
    if dest.is_dir() {
        return Ok(());
    }
    let staged = toolchain.join(format!(".new-{}", crate::text::staged_suffix()));
    // A copy whose check fails here is deleted, and builds builds the key.
    let copied = fetch(places, ssh, &line.name, key, &staged).filter(|host| match check(&staged) {
        Ok(()) => true,
        Err(error) => {
            note(format!(
                "the copy of tool {} from host {host} does not run here: {}",
                line.name,
                error.text()
            ));
            let _ = std::fs::remove_dir_all(&staged);
            false
        }
    });
    let result = match copied {
        Some(host) => {
            note(format!(
                "copied tool {} ({} {}) from host {host}",
                line.name, key.toolchain, key.sources
            ));
            Ok(())
        }
        None => build(top, line, &toolchain.join(WORK), &staged).and_then(|()| check(&staged)),
    }
    .and_then(|()| take_place(&staged, dest));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staged);
    }
    result
}

/// Runs `<program> build <staged>` in <top>, its output on stderr.
fn build(top: &Path, line: &BuiltTool, work: &Path, staged: &Path) -> Result<()> {
    std::fs::create_dir_all(work).context(|| format!("creating {}", work.display()))?;
    let program = top.join(&line.program);
    note(format!("building tool {} with {}", line.name, line.program));
    let status = Command::new(&program)
        .arg("build")
        .arg(staged)
        .env(WORK_VARIABLE, work)
        .current_dir(top)
        .stdin(Stdio::null())
        .stdout(stderr()?)
        .status()
        .context(|| format!("running {} build", program.display()))?;
    if !status.success() {
        fail!("{} build failed ({status})", program.display());
    }
    Ok(())
}

/// This process's stderr, for a child's stdout: `builds tool` prints only
/// the key's folder on stdout.
fn stderr() -> Result<Stdio> {
    use std::os::fd::AsFd;
    let fd = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context(|| "duplicating stderr".to_string())?;
    Ok(Stdio::from(fd))
}

/// Runs <folder>/check, which a build writes, on this Mac.
fn check(folder: &Path) -> Result<()> {
    let program = folder.join("check");
    let output = Command::new(&program)
        .current_dir(folder)
        .stdin(Stdio::null())
        .output()
        .context(|| format!("running {}", program.display()))?;
    if !output.status.success() {
        fail!(
            "{} failed ({}), so builds did not use the tool: {}",
            program.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Renames <staged> to <dest>. Under the toolchain's lock nothing else puts
/// <dest> in place, so a <dest> already there is an error.
fn take_place(staged: &Path, dest: &Path) -> Result<()> {
    std::fs::rename(staged, dest).context(|| format!("putting {} in place", dest.display()))
}

/// Points the symlink <link> at <dest>, in one rename.
fn point(link: &Path, dest: &Path) -> Result<()> {
    if std::fs::read_link(link).is_ok_and(|current| current == dest) {
        return Ok(());
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
    }
    let staged = link.with_file_name(format!(
        ".{}.builds-link.{}",
        link.file_name()
            .expect("a link has a name")
            .to_string_lossy(),
        crate::text::staged_suffix()
    ));
    std::os::unix::fs::symlink(dest, &staged)
        .context(|| format!("linking {}", staged.display()))?;
    std::fs::rename(&staged, link).context(|| format!("linking {}", link.display()))
}

/// The other hosts: every recorded host but this Mac. A Mac that is itself
/// a host records none.
fn other_hosts(places: &Places) -> Vec<Host> {
    match host::all(places) {
        Ok(hosts) => hosts.into_iter().filter(|h| h.target.is_some()).collect(),
        Err(error) => {
            note(format!(
                "builds could not read the hosts, so it looks for the tool on none: {}",
                error.text()
            ));
            Vec::new()
        }
    }
}

/// Copies <key> of tool <name> from the first host that has it into
/// <staged>, and returns that host's name; None when no host has it or every
/// copy failed, each failure said on stderr.
fn fetch(places: &Places, ssh: &Ssh, name: &str, key: &Key, staged: &Path) -> Option<String> {
    for host in other_hosts(places) {
        let transport = Transport {
            places,
            ssh,
            host: &host,
        };
        let remote = key_dir_on(name, key);
        let has = transport.ask(
            &format!("{}test -d {remote}", root_script(&host)),
            "looking for the tool",
        );
        if !matches!(has, Ok(Ok(_))) {
            continue;
        }
        let copied = std::fs::create_dir_all(staged)
            .context(|| format!("creating {}", staged.display()))
            .and_then(|()| {
                let sender =
                    transport.command(&format!("{}tar -cf - -C {remote} .", root_script(&host)));
                pipe(
                    sender,
                    Command::new("tar")
                        .arg("-xf")
                        .arg("-")
                        .arg("-C")
                        .arg(staged),
                )
            });
        match copied {
            Ok(()) => return Some(host.name),
            Err(error) => {
                note(format!(
                    "copying tool {name} from host {} failed: {}",
                    host.name,
                    error.text()
                ));
                let _ = std::fs::remove_dir_all(staged);
            }
        }
    }
    None
}

/// Runs <sender> with its stdout piped into <receiver>, and fails unless both
/// succeed.
fn pipe(mut sender: Command, receiver: &mut Command) -> Result<()> {
    let mut send = sender
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(|| "starting the sending side".to_string())?;
    let out = send.stdout.take().expect("piped above");
    let received = receiver
        .stdin(out)
        .stderr(Stdio::piped())
        .output()
        .context(|| "starting the receiving side".to_string())?;
    let sent = send
        .wait_with_output()
        .context(|| "waiting for the sending side".to_string())?;
    if !sent.status.success() {
        fail!(
            "the sending side failed ({}): {}",
            sent.status,
            String::from_utf8_lossy(&sent.stderr).trim()
        );
    }
    if !received.status.success() {
        fail!(
            "the receiving side failed ({}): {}",
            received.status,
            String::from_utf8_lossy(&received.stderr).trim()
        );
    }
    Ok(())
}

/// Before a job from <top> runs on <host>: copies there each key of the
/// checkout's tools that this Mac has and the host lacks, and checks it
/// there. A failure is said on stderr and leaves the host to build the key
/// when its setup runs.
pub fn send_to(places: &Places, ssh: &Ssh, host: &Host, top: &Path) {
    let tools = match built_tools(top) {
        Ok(tools) => tools,
        Err(error) => {
            note(format!(
                "builds could not read the tools to copy to host {}: {}",
                host.name,
                error.text()
            ));
            return;
        }
    };
    for line in tools {
        let sent = key(top, &line).and_then(|key| {
            let here = key_dir(places, &line.name, &key);
            if !here.is_dir() {
                return Ok(false);
            }
            send_key(places, ssh, host, &line.name, &key, &here)
        });
        match sent {
            Ok(true) => note(format!("copied tool {} to host {}", line.name, host.name)),
            Ok(false) => {}
            Err(error) => note(format!(
                "copying tool {} to host {} failed, so the host builds it: {}",
                line.name,
                host.name,
                error.text()
            )),
        }
    }
}

/// Copies <here>, this Mac's folder of <key>, to <host> unless it has the
/// key, and runs its check there before it takes its place. True when it
/// copied.
fn send_key(
    places: &Places,
    ssh: &Ssh,
    host: &Host,
    name: &str,
    key: &Key,
    here: &Path,
) -> Result<bool> {
    let transport = Transport { places, ssh, host };
    let remote = key_dir_on(name, key);
    let root = root_script(host);
    if let Ok(Ok(_)) = transport.ask(&format!("{root}test -d {remote}"), "looking for the tool") {
        return Ok(false);
    }
    let receive = format!(
        r#"{root}dest={remote}
mkdir -p "$(dirname "$dest")" || exit 1
staged=$(mktemp -d "$(dirname "$dest")/.new-XXXXXX") || exit 1
if tar -xf - -C "$staged" && (cd "$staged" && ./check); then
  [ -d "$dest" ] && rm -rf "$staged" || mv "$staged" "$dest"
else
  rm -rf "$staged"
  exit 1
fi"#
    );
    let mut sender = Command::new("tar");
    sender.arg("-cf").arg("-").arg("-C").arg(here).arg(".");
    pipe(sender, &mut transport.command(&receive))?;
    Ok(true)
}

/// A tool's toolchain folders, each with the names of its keys' folders.
type Toolchains = Vec<(String, Vec<String>)>;

/// Deletes each key's folder of every tool that no live checkout of <roots>
/// names, and each toolchain's folder that none names, work folder and all.
/// A tool whose key a checkout cannot print loses nothing.
pub fn prune_tools(places: &Places, roots: &[Root], run: Run) -> Result<()> {
    let base = places.caches().join("tools");
    let names = match listed(&base) {
        Ok(names) => names,
        Err(error) => {
            prune_listed(run, "tool builds", Err(error));
            return Ok(());
        }
    };
    if names.is_empty() {
        return Ok(());
    }
    // List the builds before reading the keys: a build put in place after
    // the listing is never deleted.
    let mut builds: Vec<(String, Toolchains)> = Vec::new();
    for name in &names {
        let mut toolchains = Vec::new();
        for toolchain in listed(&base.join(name)).map_err(Fail::Message)? {
            let sources = listed(&base.join(name).join(&toolchain)).map_err(Fail::Message)?;
            toolchains.push((toolchain, sources));
        }
        builds.push((name.clone(), toolchains));
    }
    let pairs: Vec<(PathBuf, BuiltTool)> = roots
        .iter()
        .filter_map(|root| root.live.as_ref())
        .flat_map(|checkout| {
            built_tools(&checkout.top)
                .unwrap_or_default()
                .into_iter()
                .map(|line| (checkout.top.clone(), line))
        })
        .collect();
    let keys = parallel_map(&pairs, |(top, line)| key(top, line).map_err(Fail::text));
    for (name, toolchains) in builds {
        let mut named = Vec::new();
        let mut unread = Vec::new();
        for ((top, line), key) in pairs.iter().zip(&keys) {
            if line.name != name {
                continue;
            }
            match key {
                Ok(key) => named.push(key.clone()),
                Err(error) => unread.push(format!("{}: {error}", top.display())),
            }
        }
        let what = format!("builds of tool {name} that no checkout's key names");
        if !unread.is_empty() {
            prune_listed(run, &what, Err(unread.join("; ")));
            continue;
        }
        let mut gone = Vec::new();
        for (toolchain, sources) in toolchains {
            let dir = base.join(&name).join(&toolchain);
            if !named.iter().any(|key| key.toolchain == toolchain) {
                gone.push(dir);
                continue;
            }
            for source in sources {
                if !named
                    .iter()
                    .any(|key| key.toolchain == toolchain && key.sources == source)
                {
                    gone.push(dir.join(source));
                }
            }
        }
        prune_listed(run, &what, Ok(gone));
    }
    Ok(())
}

/// The names in <dir> that don't start with a dot; none when <dir> is
/// missing.
fn listed(dir: &Path) -> std::result::Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
#[path = "built_tools_tests.rs"]
mod tests;
