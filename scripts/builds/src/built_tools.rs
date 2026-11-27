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
//! - `<program> build <folder>` builds the tool into <folder>, an empty
//!   folder builds made, and writes `<folder>/check`, a program that makes the build
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
use crate::fail;
use crate::fail::{Context, Fail, Result};
use crate::flock::lock_creating;
use crate::host::{self, Host, Ssh, sh_quote};
use crate::manifest::{BuiltTool, built_tools};
use crate::output::note;
use crate::places::Places;
use crate::prune::{Run, prune_listed};
use crate::remote::{Transport, root_script};
use crate::setup::Prepare;

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
    let env = |name: &str| std::env::var_os(name);
    let prepare = if crate::cargo_run::in_hook(&env) {
        Prepare::No
    } else {
        Prepare::Yes
    };
    let dest = ensure(places, ssh, &checkout.top, line, prepare)?;
    crate::output::answer_line(dest.display());
    Ok(())
}

/// Puts each tool <top>'s manifest declares in place and links it (setup).
pub fn ensure_all(places: &Places, ssh: &Ssh, top: &Path) -> Result<()> {
    for line in built_tools(top)? {
        ensure(places, ssh, top, &line, Prepare::Yes)?;
    }
    Ok(())
}

/// Points the checkout's link at the build of tool <line> for <top>'s key
/// on this Mac, and returns that folder. With `Prepare::Yes` a missing key
/// is put in place first, from another Mac or by building it. With
/// `Prepare::No`, for a run a pre-commit hook check started, which installs,
/// fetches and builds nothing, a missing key fails at once, naming `builds
/// setup`.
pub fn ensure(
    places: &Places,
    ssh: &Ssh,
    top: &Path,
    line: &BuiltTool,
    prepare: Prepare,
) -> Result<PathBuf> {
    let key = key(top, line)?;
    let dest = key_dir(places, &line.name, &key);
    if !dest.is_dir() {
        match prepare {
            Prepare::Yes => obtain(places, ssh, top, line, &key, &dest)?,
            Prepare::No => fail!(
                "this Mac has no build of tool {} for the checkout's key ({} {}), and a hook check builds and copies none. Run `builds setup {}` on the Mac you work on: it builds the key, or copies it from a host, and copies it to every host",
                line.name,
                key.toolchain,
                key.sources,
                top.display()
            ),
        }
    }
    point(&top.join(&line.link), &dest)?;
    Ok(dest)
}

/// Copies <key> from a host that has it, else builds it, holding the
/// toolchain folder's lock. Each copy and the build go into a staging folder
/// of their own, and a rejected one is deleted before the next starts.
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
    let _one_at_a_time = lock_creating(&toolchain.join(LOCK), libc::LOCK_EX)?;
    if dest.is_dir() {
        return Ok(());
    }
    let fetched = fetch(places, ssh, &line.name, key, toolchain)?;
    let staged = match fetched.copy {
        Some((host, staged)) => {
            note(format!(
                "copied tool {} ({} {}) from host {host}",
                line.name, key.toolchain, key.sources
            ));
            staged
        }
        None => {
            let why = match fetched.misses.as_slice() {
                [] => "no other host is recorded".to_string(),
                misses => misses.join("; "),
            };
            note(format!(
                "building tool {} with {}: {why}",
                line.name, line.program
            ));
            let staged = staging(toolchain)?;
            let built =
                build(top, line, &toolchain.join(WORK), &staged).and_then(|()| check(&staged));
            if let Err(error) = built {
                return Err(after_discard(&staged, error));
            }
            staged
        }
    };
    if let Err(error) = take_place(&staged, dest) {
        return Err(after_discard(&staged, error));
    }
    Ok(())
}

/// Makes a new, empty staging folder in <toolchain> and returns it. Its
/// name is one no earlier copy or build used, and creating it fails when
/// anything is there.
fn staging(toolchain: &Path) -> Result<PathBuf> {
    let staged = toolchain.join(format!(".new-{}", crate::text::staged_suffix()));
    std::fs::create_dir(&staged).context(|| format!("creating {}", staged.display()))?;
    Ok(staged)
}

/// <failure>, once <staged> is deleted; when the deletion fails too, an error
/// that says both.
fn after_discard(staged: &Path, failure: Fail) -> Fail {
    match discard(staged) {
        Ok(()) => failure,
        Err(cleanup) => Fail::Message(format!("{}; {}", failure.text(), cleanup.text())),
    }
}

/// Deletes <staged>, a copy or build that did not take its place; a failure
/// is an error, so nothing builds beside what it left.
fn discard(staged: &Path) -> Result<()> {
    match std::fs::remove_dir_all(staged) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => fail!(
            "deleting {}, a copy or build of a tool that did not take its place: {error}",
            staged.display()
        ),
    }
}

/// Runs `<program> build <staged>` in <top>, its output on stderr.
fn build(top: &Path, line: &BuiltTool, work: &Path, staged: &Path) -> Result<()> {
    std::fs::create_dir_all(work).context(|| format!("creating {}", work.display()))?;
    let program = top.join(&line.program);
    let status = Command::new(&program)
        .arg("build")
        .arg(staged)
        .env(WORK_VARIABLE, work)
        .current_dir(top)
        .stdin(Stdio::null())
        .stdout(crate::output::stderr_for_child()?)
        .status()
        .context(|| format!("running {} build", program.display()))?;
    if !status.success() {
        fail!("{} build failed ({status})", program.display());
    }
    Ok(())
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

/// Points the symlink <link> at <dest>, unless it points there already.
pub fn point(link: &Path, dest: &Path) -> Result<()> {
    if std::fs::read_link(link).is_ok_and(|current| current == dest) {
        return Ok(());
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).context(|| format!("creating {}", parent.display()))?;
    }
    crate::setup::replace_link(dest, link)
}

/// The `RUSTC_WORKSPACE_WRAPPER` that `builds cargo` gives cargo in <top>
/// for the caller's <wrapper>, when <wrapper> is a file under the link of
/// one of <top>'s tools: the same file in the key's build that the link
/// points at (`point` writes the build's whole path). None leaves the
/// caller's wrapper as it is.
///
/// cargo mixes the wrapper's path into the name of each workspace crate's
/// output, and replays the last warnings of a crate whose sources did not
/// change. Through the link, the path stays the same when the link moves to
/// another key's build, so cargo would replay the lints of the build the
/// link pointed at before. With the key's own path, each key's lints get
/// outputs of their own: another key lints every crate again, and a second
/// run of the same key replays that key's results.
pub fn cargo_workspace_wrapper(top: &Path, wrapper: &Path) -> Result<Option<PathBuf>> {
    for line in built_tools(top)? {
        let link = top.join(&line.link);
        let Ok(within) = wrapper.strip_prefix(&link) else {
            continue;
        };
        let build = std::fs::read_link(&link).context(|| {
            format!(
                "RUSTC_WORKSPACE_WRAPPER names {}, under tool {}'s link, and the link points at no build (`builds tool {}` points it)",
                wrapper.display(),
                line.name,
                line.name
            )
        })?;
        return Ok(Some(build.join(within)));
    }
    Ok(None)
}

/// The other hosts: every recorded host but this Mac. A Mac that is itself
/// a host records none.
fn other_hosts(places: &Places) -> Result<Vec<Host>> {
    Ok(host::all(places)?
        .into_iter()
        .filter(|host| host.target.is_some())
        .collect())
}

/// Does <host> have <remote>, a key's folder there? None when the host is
/// of another platform than this machine (host_platform.rs): a build of a
/// tool runs only on its own platform, so builds copies it only between
/// hosts of one platform, and each other platform builds its own. An error
/// says why builds could not ask: the host did not answer, or the question
/// failed there.
fn host_has(transport: &Transport, host: &Host, remote: &str) -> Result<Option<bool>> {
    let script = format!(
        "{}{}\nif [ -d {remote} ]; then echo present=yes; else echo present=no; fi",
        root_script(host),
        crate::host_platform::SCRIPT
    );
    let said = transport
        .ask(&script, "looking for the tool")?
        .map_err(Fail::Message)?;
    if crate::host_platform::Platform::parse(&said)? != crate::host_platform::Platform::here() {
        return Ok(None);
    }
    match host::answer_value(&said, "present") {
        Some("yes") => Ok(Some(true)),
        Some("no") => Ok(Some(false)),
        _ => fail!(
            "host {} answered {said:?} when asked for the tool",
            host.name
        ),
    }
}

/// What `fetch` found: the host it copied the key from and the staging
/// folder that holds the copy, and why each other host gave none.
struct Fetched {
    copy: Option<(String, PathBuf)>,
    misses: Vec<String>,
}

/// Copies <key> of tool <name> from the first host that has it into a new
/// staging folder in <toolchain> and checks it here. A host that lacks the
/// key, one builds could not ask, and a copy that failed or does not run here
/// each add their reason to the misses; a rejected copy is deleted, and a
/// failed deletion is an error.
fn fetch(places: &Places, ssh: &Ssh, name: &str, key: &Key, toolchain: &Path) -> Result<Fetched> {
    let mut misses = Vec::new();
    let hosts = match other_hosts(places) {
        Ok(hosts) => hosts,
        Err(error) => {
            misses.push(format!("builds could not read the hosts: {}", error.text()));
            Vec::new()
        }
    };
    for host in hosts {
        let transport = Transport {
            places,
            ssh,
            host: &host,
        };
        let remote = key_dir_on(name, key);
        match host_has(&transport, &host, &remote) {
            Ok(Some(true)) => {}
            Ok(Some(false)) => {
                misses.push(format!("host {} does not have it", host.name));
                continue;
            }
            Ok(None) => {
                misses.push(format!(
                    "host {} is of another platform than this machine",
                    host.name
                ));
                continue;
            }
            Err(error) => {
                misses.push(format!(
                    "builds could not ask host {} for it: {}",
                    host.name,
                    error.text()
                ));
                continue;
            }
        }
        let staged = staging(toolchain)?;
        let sender = transport.command(&format!("{}tar -cf - -C {remote} .", root_script(&host)));
        let copied = crate::remote::unpack(
            sender,
            &staged,
            &format!("tool {name} from host {}", host.name),
        )
        .and_then(|()| check(&staged));
        match copied {
            Ok(()) => {
                return Ok(Fetched {
                    copy: Some((host.name, staged)),
                    misses,
                });
            }
            Err(error) => {
                misses.push(format!(
                    "the copy from host {} failed: {}",
                    host.name,
                    error.text()
                ));
                if let Err(cleanup) = discard(&staged) {
                    fail!("{}; {}", misses.join("; "), cleanup.text());
                }
            }
        }
    }
    Ok(Fetched { copy: None, misses })
}

/// Copies each key of <top>'s tools that this Mac has to every host that
/// lacks it (setup), each failed copy said on stderr. A manifest that can't
/// be read is an error.
pub fn send_to_hosts(places: &Places, ssh: &Ssh, top: &Path) -> Result<()> {
    if built_tools(top)?.is_empty() {
        return Ok(());
    }
    match other_hosts(places) {
        Ok(hosts) => {
            for host in hosts {
                send_to(places, ssh, &host, top);
            }
        }
        Err(error) => note(format!(
            "builds could not read the hosts, so it copied no tool to them: {}",
            error.text()
        )),
    }
    Ok(())
}

/// Before a job from <top> runs on <host>: copies there each key of the
/// checkout's tools that this Mac has and the host lacks, when the host is
/// of this Mac's platform, and checks it there. A failure is said on stderr and leaves the host to build the key
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
    let transport = Transport { places, ssh, host };
    for line in tools {
        let sent = key(top, &line).and_then(|key| {
            let here = key_dir(places, &line.name, &key);
            let remote = key_dir_on(&line.name, &key);
            if !here.is_dir() || host_has(&transport, host, &remote)? != Some(false) {
                return Ok(false);
            }
            send_key(&transport, host, &line.name, &remote, &here)?;
            Ok(true)
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

/// Copies <here>, this Mac's folder of a key of tool <name>, to <remote>, its
/// folder on <host>: into a new staging folder there, whose check must pass
/// there before it takes its place. A staging folder whose copy or check
/// failed is deleted.
fn send_key(
    transport: &Transport,
    host: &Host,
    name: &str,
    remote: &str,
    here: &Path,
) -> Result<()> {
    let receive = format!(
        r#"{root}dest={remote}
mkdir -p "$(dirname "$dest")" || exit 1
staged=$(mktemp -d "$(dirname "$dest")/.new-XXXXXX") || exit 1
if tar -xf - -C "$staged" && (cd "$staged" && ./check); then
  if [ -d "$dest" ]; then
    rm -rf "$staged"
  else
    mv "$staged" "$dest"
  fi
else
  rm -rf "$staged" || echo "deleting $staged failed" >&2
  exit 1
fi"#,
        root = root_script(host)
    );
    let mut sender = Command::new("tar");
    sender.arg("-cf").arg("-").arg("-C").arg(here).arg(".");
    crate::remote::pipe(
        sender,
        transport.command(&receive),
        &format!("tool {name} to host {}", host.name),
    )
}

/// A tool's toolchain folders, each with the names of its keys' folders.
type Toolchains = Vec<(String, Vec<String>)>;

/// Deletes each key's folder of every tool that no live checkout of <roots>
/// names, and each toolchain's folder that none names, work folder and all.
/// A tool whose key a checkout cannot print loses nothing, and no tool loses
/// anything while a checkout's manifest cannot be read.
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
    // A checkout whose manifest can't be read may name any tool, so then
    // no tool loses a build.
    let mut pairs: Vec<(PathBuf, BuiltTool)> = Vec::new();
    let mut unread = Vec::new();
    for checkout in roots.iter().filter_map(|root| root.live.as_ref()) {
        match built_tools(&checkout.top) {
            Ok(lines) => pairs.extend(lines.into_iter().map(|line| (checkout.top.clone(), line))),
            Err(error) => unread.push(error.text()),
        }
    }
    if !unread.is_empty() {
        prune_listed(run, "tool builds", Err(unread.join("; ")));
        return Ok(());
    }
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
