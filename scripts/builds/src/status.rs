//! `builds status`: every entry on the volume, with size against cap, last
//! write and links, and build output in the temporary folders; `--json` the
//! repos' checkouts and the volume's free space.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::checkout::{Root, absolute_link_target, file_name, parallel_map};
use crate::claim::Claim;
use crate::fail;
use crate::fail::Result;
use crate::install::sccache_config_holds;
use crate::lane::{Lane, is_lane_file};
use crate::manifest::{SHARED_CACHES, build_dirs, kept_builds};
use crate::places::{DERIVED_DATA_CAP_KIB, FLOOR_IDLE_MINUTES, MACOS_ENTRIES, DiskSpace, Places};
use crate::prune::derived_data;
use crate::temp_builds::temp_build_folders;
use crate::text::{glob_names, human, last_write, tilde};
use crate::tool::{Tool, volume_space};
use crate::trees::{Tree, trees_in, unused};

/// What status leaves out at the volume's root: macOS's own entries and
/// the daemon's folder.
fn left_out(name: &str) -> bool {
    MACOS_ENTRIES.contains(&name) || name == ".builds-daemon"
}

/// What a repo's folder on the volume holds, in glob order.
pub enum RepoEntry {
    Root(Root),
    /// Anything builds did not make there.
    Unknown(PathBuf),
}

pub struct Repo {
    pub name: String,
    pub dir: PathBuf,
    pub entries: Vec<RepoEntry>,
}

/// Is <dir> a repo's folder: checkout roots, its app checkout's name or
/// lock?
fn is_repo_dir(dir: &Path) -> Result<bool> {
    if !dir.is_dir() || dir.is_symlink() {
        return Ok(false);
    }
    if Lane::ALL.into_iter().any(|lane| dir.join(lane.checkout_file_name()).is_file() || dir.join(lane.lock_file_name()).is_file()) {
        return Ok(true);
    }
    let names = match glob_names(dir) {
        Ok(names) => names,
        // A folder this user can't list holds no roots of theirs.
        Err(_) if std::fs::read_dir(dir).is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied) => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    for name in names {
        if name.as_encoded_bytes().first() != Some(&b'.') && dir.join(&name).join(".checkout").is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_root_dir(path: &Path) -> bool {
    path.is_dir() && !path.is_symlink() && path.join(".checkout").is_file()
}

/// Every repo folder on the volume with its entries, roots read (git runs
/// for each, several at a time).
pub fn repos(places: &Places) -> Result<Vec<Repo>> {
    let mut found = Vec::new();
    let mut root_paths = Vec::new();
    for name in glob_names(&places.root)? {
        let dir = places.root.join(&name);
        if left_out(&name.to_string_lossy()) {
            continue;
        }
        if dir == places.trash() || dir == places.derived_data() || dir == places.caches() {
            continue;
        }
        if !is_repo_dir(&dir)? {
            continue;
        }
        let mut paths = Vec::new();
        for entry in glob_names(&dir)? {
            let path = dir.join(&entry);
            if is_lane_file(&entry.to_string_lossy()) && path.is_file() {
                continue;
            }
            if is_root_dir(&path) {
                root_paths.push(path.clone());
            }
            paths.push(path);
        }
        found.push((name.to_string_lossy().into_owned(), dir, paths));
    }
    let read = parallel_map(&root_paths, |path| Root::read(places, path));
    let mut roots: Vec<(PathBuf, Root)> = Vec::new();
    for (path, root) in root_paths.into_iter().zip(read) {
        if let Some(root) = root? {
            roots.push((path, root));
        }
    }
    Ok(found
        .into_iter()
        .map(|(name, dir, paths)| Repo {
            name,
            dir,
            entries: paths
                .into_iter()
                .map(|path| match roots.iter().position(|(p, _)| *p == path) {
                    Some(at) => RepoEntry::Root(roots[at].1.clone()),
                    None => RepoEntry::Unknown(path),
                })
                .collect(),
        })
        .collect())
}

/// The paths status measures: each live checkout's existing build folders,
/// and each gone one's root.
fn status_folders(repos: &[Repo]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for repo in repos {
        for entry in &repo.entries {
            match entry {
                RepoEntry::Root(root) if root.live.is_some() => {
                    for dir in build_dirs(&root.top)? {
                        let folder = root.path.join(&dir.path);
                        if folder.is_dir() {
                            paths.extend(trees_in(&folder)?.into_iter().map(|tree| tree.path));
                            paths.push(folder);
                        }
                    }
                }
                RepoEntry::Root(root) => paths.push(root.path.clone()),
                RepoEntry::Unknown(path) => paths.push(path.clone()),
            }
        }
    }
    Ok(paths)
}

fn relative(tool: &Tool, path: &Path) -> String {
    path.strip_prefix(&tool.places.root).unwrap_or(path).display().to_string()
}

fn unknown_line(tool: &Tool, path: &Path) -> Result<String> {
    Ok(format!(
        "{}  {}  UNKNOWN: builds made nothing here",
        relative(tool, path),
        human(tool.measure(path)?.kib)
    ))
}

/// <tree>'s path inside <folder>, as status names it.
fn tree_name(folder: &Path, tree: &Tree) -> String {
    tree.path.strip_prefix(folder).unwrap_or(&tree.path).display().to_string()
}

fn root_lines(tool: &Tool, root: &Root, out: &mut Vec<String>) -> Result<()> {
    let Some(checkout) = &root.live else {
        out.push(format!(
            "{}  {}  [checkout gone: prune deletes this root]",
            relative(tool, &root.path),
            root.top.display()
        ));
        out.push(format!("    {} total", human(tool.measure(&root.path)?.kib)));
        return Ok(());
    };
    let mut state = checkout.branch()?.unwrap_or_else(|| "detached".to_string());
    if let Some(lane) = root.app_lane(&tool.places)? {
        state += &format!(", {} app checkout", lane.word());
    }
    let claim = Claim::read(&checkout.claim_file())?;
    if let Some(claim) = &claim {
        state += &format!(", claimed by {} since {}", claim.who, claim.since);
    }
    out.push(format!("{}  {}  [{state}]", relative(tool, &root.path), root.top.display()));
    if let Some(claim) = &claim {
        match (&claim.note_state, &claim.note) {
            (Some(state), note) => out.push(format!(
                "    note ({}): {}",
                state.replace('-', " "),
                note.as_deref().unwrap_or("")
            )),
            (None, Some(note)) => out.push(format!("    note: {note}")),
            (None, None) => {}
        }
    }
    let mut total = 0;
    let kept: Vec<PathBuf> = if root.keeps_warm(&tool.places)? {
        kept_builds(&root.top)?.iter().map(|path| root.path.join(path)).collect()
    } else {
        Vec::new()
    };
    let lane = root.app_lane(&tool.places)?;
    for entry in build_dirs(&root.top)? {
        let (path, cap_kib) = (&entry.path, entry.cap_kib(entry.is_app_in(lane)));
        let folder = root.path.join(path);
        if !folder.is_dir() {
            out.push(format!("    {path:<34} {:>8} / {:<6}  (no folder)", "-", human(cap_kib)));
            continue;
        }
        let measured = tool.measure(&folder)?;
        total += measured.kib;
        let over = if measured.kib > cap_kib { "OVER CAP" } else { "" };
        out.push(format!(
            "    {path:<34} {:>8} / {:<6}  last write {}  {over}",
            human(measured.kib),
            human(cap_kib),
            last_write(measured.newest)
        ));
        for tree in trees_in(&folder)? {
            let measured = tool.measure(&tree.path)?;
            let state = if kept.contains(&tree.path) {
                "kept: the person runs it".to_string()
            } else if unused(tool, &measured) {
                format!("unused for {} days: setup deletes it", tool.rules.unused_days)
            } else {
                String::new()
            };
            out.push(format!(
                "      {:<32} {:>8}           last use {}  {state}",
                tree_name(&folder, &tree),
                human(measured.kib),
                last_write(measured.newest)
            ));
        }
        let link = root.top.join(path);
        if link.is_symlink() {
            if absolute_link_target(&link)? != folder {
                out.push(format!(
                    "      link points elsewhere: {}",
                    std::fs::read_link(&link).map(|t| t.display().to_string()).unwrap_or_default()
                ));
            }
        } else {
            out.push(format!("      not linked: run builds setup {}", root.top.display()));
        }
    }
    out.push(format!("    {} total", human(total)));
    Ok(())
}

/// Where Xcode puts DerivedData by default, as `defaults` reads it.
fn xcode_derived_data() -> Option<String> {
    let output = Command::new("defaults")
        .args(["read", "com.apple.dt.Xcode", "IDECustomDerivedDataLocation"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !text.is_empty() { Some(text) } else { None }
}

fn derived_data_lines(tool: &Tool, out: &mut Vec<String>) -> Result<()> {
    let dir = tool.places.derived_data();
    let cap = DERIVED_DATA_CAP_KIB;
    let (measured, entries) = derived_data(tool)?;
    let kib = measured.kib;
    let newest = entries.last().map(|entry| entry.newest).unwrap_or(0);
    let over = if kib > cap { "OVER CAP" } else { "" };
    out.push(format!(
        "{:<38} {:>8} / {:<6}  last write {}  {over}",
        "DerivedData (Xcode's default)",
        human(kib),
        human(cap),
        last_write(Some(newest))
    ));
    for entry in &entries {
        out.push(format!(
            "    {:<34} {:>8}           last write {}",
            file_name(&entry.path),
            human(entry.kib),
            last_write(Some(entry.newest))
        ));
    }
    let xcode = xcode_derived_data();
    if xcode.as_deref() != Some(dir.to_string_lossy().as_ref()) {
        out.push(format!(
            "    Xcode's default DerivedData is {}, not this: run builds install",
            xcode.unwrap_or_else(|| "~/Library/Developer/Xcode/DerivedData".to_string())
        ));
    }
    Ok(())
}

fn caches_lines(tool: &Tool, out: &mut Vec<String>) -> Result<()> {
    let caches = tool.places.caches();
    let home = &tool.places.home;
    out.push("caches  (shared by every checkout)".to_string());
    for cache in SHARED_CACHES {
        let dir = caches.join(cache.name);
        if !dir.is_dir() {
            out.push(format!(
                "    {:<34} {:>8} / {:<6}  (no folder: run builds install)",
                cache.name,
                "-",
                human(cache.cap_kib)
            ));
        } else {
            let measured = tool.measure(&dir)?;
            let state = if cache.name == "sccache" && !sccache_config_holds(home, cache.cap_kib)? {
                format!(
                    "CONFIG DIFFERS: {} does not hold this cap; run builds install",
                    tilde(&crate::install::sccache_config_path(home), home)
                )
            } else if cache.name == "sccache" {
                "(sccache keeps it under its own cap)".to_string()
            } else if measured.kib > cache.cap_kib {
                "OVER CAP".to_string()
            } else {
                String::new()
            };
            out.push(format!(
                "    {:<34} {:>8} / {:<6}  last write {}  {state}",
                cache.name,
                human(measured.kib),
                human(cache.cap_kib),
                last_write(measured.newest)
            ));
        }
        let link = cache.link_path(home);
        let shown = tilde(&link, home);
        if link.is_symlink() {
            if absolute_link_target(&link)? != dir {
                out.push(format!(
                    "      {shown} points elsewhere: {}",
                    std::fs::read_link(&link).map(|t| t.display().to_string()).unwrap_or_default()
                ));
            }
        } else if link.exists() {
            out.push(format!("      {shown} is a real directory, not a link here: run builds install"));
        } else {
            out.push(format!("      {shown} is missing: run builds install"));
        }
    }
    for name in glob_names(&caches)? {
        let name = name.to_string_lossy();
        if !SHARED_CACHES.iter().any(|cache| cache.name == name) {
            out.push(unknown_line(tool, &caches.join(name.as_ref()))?);
        }
    }
    Ok(())
}

/// `sccache: ...` lines from a running server.
fn sccache_lines(out: &mut Vec<String>) {
    if !crate::sccache::listening() {
        return;
    }
    for line in crate::sccache::stats() {
        if line.starts_with("Cache location") || line.starts_with("Cache size") || line.starts_with("Max cache size") {
            out.push(format!("sccache: {line}"));
        }
    }
}

/// `volume: 52Gi free of 460Gi`, as `df -h` says them.
fn volume_line(root: &Path) -> Result<String> {
    let output = Command::new("df").arg("-h").arg(root).output().map_err(|error| {
        fail::Fail::Message(format!("running df: {error}"))
    })?;
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(fields) = text.lines().nth(1).map(|line| line.split_whitespace().collect::<Vec<_>>()) else {
        fail!("df -h {} printed {text:?}", root.display());
    };
    if fields.len() < 4 {
        fail!("df -h {} printed {text:?}", root.display());
    }
    Ok(format!("volume: {} free of {}", fields[3], fields[1]))
}

/// `builds status [--json [--no-folders]]`.
pub fn cmd_status(tool: &mut Tool, args: &[String]) -> Result<()> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {}
        ["--json"] => return print_json(tool, true),
        ["--json", "--no-folders"] | ["--no-folders", "--json"] => return print_json(tool, false),
        _ => fail!("usage: builds status [--json [--no-folders]]"),
    }
    tool.places.require_volume()?;
    let repos = repos(&tool.places)?;
    let temps = temp_build_folders(&tool.places)?;
    let mut ahead = status_folders(&repos)?;
    ahead.extend(temps.iter().cloned());
    for name in glob_names(&tool.places.root)? {
        if !left_out(&name.to_string_lossy()) {
            ahead.push(tool.places.root.join(name));
        }
    }
    for name in glob_names(&tool.places.derived_data())? {
        ahead.push(tool.places.derived_data().join(name));
    }
    for name in glob_names(&tool.places.caches())? {
        ahead.push(tool.places.caches().join(name));
    }
    tool.measure_ahead(ahead)?;
    let mut out = Vec::new();
    for name in glob_names(&tool.places.root)? {
        let entry = tool.places.root.join(&name);
        if left_out(&name.to_string_lossy()) {
            continue;
        }
        if entry == tool.places.trash() && entry.is_dir() {
            let holds_any = std::fs::read_dir(&entry)
                .map_err(|error| fail::Fail::Message(format!("listing {}: {error}", entry.display())))?
                .next()
                .is_some();
            if holds_any {
                out.push(format!(
                    ".trash  {}  (deletions finishing in the background)",
                    human(tool.measure(&entry)?.kib)
                ));
            }
        } else if entry == tool.places.derived_data() && entry.is_dir() {
            derived_data_lines(tool, &mut out)?;
        } else if entry == tool.places.caches() && entry.is_dir() {
            caches_lines(tool, &mut out)?;
        } else if let Some(repo) = repos.iter().find(|repo| repo.dir == entry) {
            for item in &repo.entries {
                match item {
                    RepoEntry::Root(root) => root_lines(tool, root, &mut out)?,
                    RepoEntry::Unknown(path) => out.push(unknown_line(tool, path)?),
                }
            }
        } else {
            out.push(unknown_line(tool, &entry)?);
        }
    }
    for folder in &temps {
        let measured = tool.measure(folder)?;
        let state = if tool.written_within(&measured, FLOOR_IDLE_MINUTES) {
            format!("written in the last {FLOOR_IDLE_MINUTES}m")
        } else {
            "prune deletes it".to_string()
        };
        out.push(format!(
            "temp build output  {}  {}  last write {}  ({state})",
            folder.display(),
            human(measured.kib),
            last_write(measured.newest)
        ));
    }
    if !tool.places.derived_data().is_dir() {
        out.push("DerivedData  (no folder: run builds install)".to_string());
    }
    if !tool.places.caches().is_dir() {
        out.push("caches  (no folder: run builds install)".to_string());
    }
    sccache_lines(&mut out);
    out.push(volume_line(&tool.places.root)?);
    for line in out {
        println!("{line}");
    }
    Ok(())
}

// ── JSON ───────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StatusJson {
    pub root: String,
    pub volume: VolumeJson,
    pub repos: Vec<RepoJson>,
}

#[derive(Serialize)]
pub struct VolumeJson {
    #[serde(flatten)]
    pub disk: DiskSpace,
    pub size_kib: u64,
}

#[derive(Serialize)]
pub struct RepoJson {
    pub name: String,
    /// Each app lane that has a checkout recorded or a lock.
    pub lanes: Vec<LaneJson>,
    pub checkouts: Vec<CheckoutJson>,
}

#[derive(Serialize)]
pub struct LaneJson {
    pub lane: &'static str,
    pub app_checkout: Option<String>,
    pub app_lock: Option<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum CheckoutJson {
    Live {
        name: String,
        path: String,
        root: String,
        live: bool,
        main: bool,
        branch: Option<String>,
        app_checkout: bool,
        /// The lane it is the app checkout of.
        app_lane: Option<&'static str>,
        claim: Option<Claim>,
        folders: Option<Vec<FolderJson>>,
        total_kib: Option<u64>,
    },
    Gone {
        name: String,
        path: String,
        root: String,
        live: bool,
        total_kib: Option<u64>,
    },
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum FolderJson {
    Missing {
        path: String,
        cap_kib: u64,
        exists: bool,
    },
    Present {
        path: String,
        cap_kib: u64,
        exists: bool,
        kib: u64,
        last_write: Option<i64>,
        link: &'static str,
        link_target: Option<String>,
        trees: Vec<TreeJson>,
    },
}

/// A build inside a build folder (see trees.rs): its path inside the folder,
/// its tool, its size and when a build last used it.
#[derive(Serialize)]
pub struct TreeJson {
    pub path: String,
    pub kind: &'static str,
    pub kib: u64,
    pub last_use: Option<i64>,
}

fn checkout_json(tool: &Tool, root: &Root, with_folders: bool) -> Result<CheckoutJson> {
    let name = root.name();
    let path = root.top.display().to_string();
    let root_path = root.path.display().to_string();
    let Some(checkout) = &root.live else {
        let total_kib = if with_folders { Some(tool.measure(&root.path)?.kib) } else { None };
        return Ok(CheckoutJson::Gone { name, path, root: root_path, live: false, total_kib });
    };
    let mut folders = None;
    let mut total_kib = None;
    if with_folders {
        let mut list = Vec::new();
        let mut total = 0;
        let lane = root.app_lane(&tool.places)?;
        for entry in build_dirs(&root.top)? {
            let cap_kib = entry.cap_kib(entry.is_app_in(lane));
            let path = entry.path;
            let folder = root.path.join(&path);
            if !folder.is_dir() {
                list.push(FolderJson::Missing { path, cap_kib, exists: false });
                continue;
            }
            let measured = tool.measure(&folder)?;
            total += measured.kib;
            let link_path = root.top.join(&path);
            let (link, link_target) = if link_path.is_symlink() {
                let target = absolute_link_target(&link_path)?;
                (if target == folder { "linked" } else { "elsewhere" }, Some(target.display().to_string()))
            } else {
                ("not linked", None)
            };
            let mut trees = Vec::new();
            for tree in trees_in(&folder)? {
                let measured = tool.measure(&tree.path)?;
                trees.push(TreeJson {
                    path: tree_name(&folder, &tree),
                    kind: tree.kind.name(),
                    kib: measured.kib,
                    last_use: measured.newest,
                });
            }
            list.push(FolderJson::Present {
                path,
                cap_kib,
                exists: true,
                kib: measured.kib,
                last_write: measured.newest,
                link,
                link_target,
                trees,
            });
        }
        folders = Some(list);
        total_kib = Some(total);
    }
    Ok(CheckoutJson::Live {
        name: name.clone(),
        path,
        root: root_path,
        live: true,
        main: name == "main",
        branch: checkout.branch()?,
        app_checkout: root.is_app_checkout(&tool.places)?,
        app_lane: root.app_lane(&tool.places)?.map(Lane::word),
        claim: Claim::read(&checkout.claim_file())?,
        folders,
        total_kib,
    })
}

/// <repo>'s lanes that have an app checkout recorded or a lock.
pub fn lanes_of(places: &Places, repo: &str) -> Result<Vec<LaneJson>> {
    let mut lanes = Vec::new();
    for lane in Lane::ALL {
        let (app_checkout, lock) = (places.app_checkout_of(repo, lane)?, places.app_lock_file(repo, lane));
        if app_checkout.is_some() || lock.is_file() {
            lanes.push(LaneJson { lane: lane.word(), app_checkout, app_lock: lock.is_file().then(|| lock.display().to_string()) });
        }
    }
    Ok(lanes)
}

/// The repos on the volume, each with its checkouts in the order status
/// lists them, and the volume's free space against the floors.
pub fn status_json(tool: &mut Tool, with_folders: bool) -> Result<StatusJson> {
    tool.places.require_volume()?;
    let repos = repos(&tool.places)?;
    if with_folders {
        let ahead = status_folders(&repos)?;
        tool.measure_ahead(ahead)?;
    }
    let space = volume_space(&tool.places.root)?;
    let mut list = Vec::new();
    for repo in &repos {
        let roots: Vec<&Root> = repo
            .entries
            .iter()
            .filter_map(|entry| match entry {
                RepoEntry::Root(root) => Some(root),
                RepoEntry::Unknown(_) => None,
            })
            .collect();
        let checkouts = parallel_map(&roots, |root| checkout_json(tool, root, with_folders));
        list.push(RepoJson {
            name: repo.name.clone(),
            lanes: lanes_of(&tool.places, &repo.name)?,
            checkouts: checkouts.into_iter().collect::<Result<_>>()?,
        });
    }
    Ok(StatusJson {
        root: tool.places.root.display().to_string(),
        volume: VolumeJson {
            disk: tool.rules.disk_space(space.free_kib),
            size_kib: space.size_kib,
        },
        repos: list,
    })
}

fn print_json(tool: &mut Tool, with_folders: bool) -> Result<()> {
    let status = status_json(tool, with_folders)?;
    println!("{}", serde_json::to_string(&status).expect("status serializes"));
    Ok(())
}
