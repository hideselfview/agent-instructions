//! The machine's processes, read through libproc: names, parents, start
//! times, working directories, arguments, CPU time, memory and open files.
//! One `Processes` is one pass over every process; what costs more than the
//! name is read only for the processes a caller asks about.

use std::collections::HashMap;
use std::ffi::CStr;
use std::path::PathBuf;

/// Names of the processes that build: compilers, linkers and build tools.
pub const BUILD_NAMES: [&str; 17] = [
    "cargo",
    "rustc",
    "clippy-driver",
    "rustdoc",
    "sccache",
    "cc",
    "clang",
    "ld",
    "xcodebuild",
    "swift-frontend",
    "swiftc",
    "swift-build",
    "java",
    "lake",
    "lean",
    "dotnet",
    "cargo-nextest",
];

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    /// The short name the kernel keeps (up to 32 bytes).
    pub name: String,
    /// Seconds since 1970.
    pub started: i64,
}

pub struct Processes {
    pub list: Vec<Process>,
    by_pid: HashMap<i32, usize>,
}

fn c_text(bytes: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = bytes.iter().take_while(|b| **b != 0).map(|b| *b as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

impl Processes {
    /// One pass over every process this user can read.
    pub fn read() -> Processes {
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let mut pids = vec![0i32; (count.max(0) as usize) + 256];
        let bytes = (pids.len() * std::mem::size_of::<i32>()) as libc::c_int;
        let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(filled.max(0) as usize);
        let mut list = Vec::with_capacity(pids.len());
        for pid in pids {
            if pid == 0 {
                continue;
            }
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let read = unsafe {
                libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size)
            };
            // A process that exited since it was listed, or one this user
            // may not read, has nothing to say.
            if read != size {
                continue;
            }
            let name = c_text(&info.pbi_name);
            list.push(Process {
                pid,
                ppid: info.pbi_ppid as i32,
                uid: info.pbi_uid,
                name: if name.is_empty() { c_text(&info.pbi_comm) } else { name },
                started: info.pbi_start_tvsec as i64,
            });
        }
        let by_pid = list.iter().enumerate().map(|(at, p)| (p.pid, at)).collect();
        Processes { list, by_pid }
    }

    pub fn get(&self, pid: i32) -> Option<&Process> {
        self.by_pid.get(&pid).map(|at| &self.list[*at])
    }

    /// The direct children of <pid>.
    pub fn children(&self, pid: i32) -> impl Iterator<Item = &Process> {
        self.list.iter().filter(move |p| p.ppid == pid)
    }

    /// <pid> and its ancestors, nearest first.
    pub fn ancestry(&self, pid: i32) -> Vec<&Process> {
        let mut chain = Vec::new();
        let mut at = self.get(pid);
        while let Some(process) = at {
            if chain.iter().any(|p: &&Process| p.pid == process.pid) {
                break;
            }
            chain.push(process);
            at = if process.ppid > 0 { self.get(process.ppid) } else { None };
        }
        chain
    }
}

/// The working directory of <pid>; None when it exited or can't be read.
pub fn cwd(pid: i32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size)
    };
    if read != size {
        return None;
    }
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    let path = path.to_string_lossy();
    if path.is_empty() { None } else { Some(PathBuf::from(path.as_ref())) }
}

/// The arguments <pid> was started with (KERN_PROCARGS2); None when it exited
/// or can't be read.
pub fn args(pid: i32) -> Option<Vec<String>> {
    let mut size: libc::size_t = 0;
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut argmax: libc::c_int = 0;
    let mut argmax_size = std::mem::size_of::<libc::c_int>();
    if unsafe {
        libc::sysctl(mib.as_mut_ptr(), 2, (&mut argmax as *mut libc::c_int).cast(), &mut argmax_size, std::ptr::null_mut(), 0)
    } != 0
    {
        return None;
    }
    let mut buffer = vec![0u8; argmax as usize];
    size = size.max(buffer.len());
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buffer.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    buffer.truncate(size);
    if buffer.len() < 4 {
        return None;
    }
    let argc = i32::from_ne_bytes(buffer[..4].try_into().ok()?) as usize;
    let mut rest = &buffer[4..];
    // The executable's path, then NULs up to the arguments.
    let end = rest.iter().position(|b| *b == 0)?;
    rest = &rest[end..];
    let start = rest.iter().position(|b| *b != 0)?;
    rest = &rest[start..];
    let mut found = Vec::with_capacity(argc);
    for part in rest.split(|b| *b == 0).take(argc) {
        found.push(String::from_utf8_lossy(part).into_owned());
    }
    Some(found)
}

/// CPU time used so far, in seconds, and the physical footprint in bytes.
pub fn usage(pid: i32) -> Option<(f64, u64)> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&mut info as *mut libc::rusage_info_v2).cast())
    };
    if rc != 0 {
        return None;
    }
    let ticks = info.ri_user_time + info.ri_system_time;
    Some((ticks as f64 * timebase() / 1e9, info.ri_phys_footprint))
}

/// <mach/mach_time.h>'s struct mach_timebase_info.
#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> libc::c_int;
}

/// Nanoseconds per Mach absolute-time tick.
fn timebase() -> f64 {
    static TIMEBASE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        unsafe { mach_timebase_info(&mut info) };
        if info.denom == 0 { 1.0 } else { info.numer as f64 / info.denom as f64 }
    })
}

/// sys/proc_info.h's struct proc_fileinfo and vnode_fdinfowithpath.
#[repr(C)]
struct ProcFileInfo {
    openflags: u32,
    status: u32,
    offset: i64,
    kind: i32,
    guardflags: u32,
}

#[repr(C)]
struct VnodeFdInfoWithPath {
    pfi: ProcFileInfo,
    pvip: libc::vnode_info_path,
}

const PROC_PIDFDVNODEPATHINFO: libc::c_int = 2;

/// The paths of the files <pid> has open.
pub fn open_files(pid: i32) -> Vec<PathBuf> {
    let needed = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let slots = needed as usize / std::mem::size_of::<libc::proc_fdinfo>() + 16;
    let mut fds: Vec<libc::proc_fdinfo> = (0..slots).map(|_| libc::proc_fdinfo { proc_fd: 0, proc_fdtype: 0 }).collect();
    let bytes = (fds.len() * std::mem::size_of::<libc::proc_fdinfo>()) as libc::c_int;
    let filled = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), bytes) };
    if filled <= 0 {
        return Vec::new();
    }
    fds.truncate(filled as usize / std::mem::size_of::<libc::proc_fdinfo>());
    let mut paths = Vec::new();
    for fd in fds {
        if fd.proc_fdtype as libc::c_int != libc::PROX_FDTYPE_VNODE {
            continue;
        }
        let mut info: VnodeFdInfoWithPath = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<VnodeFdInfoWithPath>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidfdinfo(pid, fd.proc_fd, PROC_PIDFDVNODEPATHINFO, (&mut info as *mut VnodeFdInfoWithPath).cast(), size)
        };
        if read != size {
            continue;
        }
        let path = unsafe { CStr::from_ptr(info.pvip.vip_path.as_ptr().cast()) };
        paths.push(PathBuf::from(path.to_string_lossy().as_ref()));
    }
    paths
}

/// The 1-, 5- and 15-minute load averages.
pub fn load_average() -> [f64; 3] {
    let mut loads = [0f64; 3];
    unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
    loads
}

/// The number of CPU cores.
pub fn cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// A checkout processes are placed in: its working tree and its build root.
#[derive(Clone, Debug)]
pub struct Place {
    /// `bae/side-3`.
    pub label: String,
    pub top: PathBuf,
    pub root: PathBuf,
}

/// A build process at work in a checkout.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BuildProcess {
    pub pid: i32,
    pub name: String,
    /// Index into the places it was placed among.
    #[serde(skip)]
    pub place: usize,
    pub started: i64,
    /// CPU time so far, seconds.
    pub cpu_seconds: f64,
    pub footprint: u64,
}

/// Does the text <text> name <path> or something inside it?
fn mentions(text: &str, path: &str) -> bool {
    let mut from = 0;
    while let Some(at) = text[from..].find(path) {
        let end = from + at + path.len();
        if matches!(text.as_bytes().get(end), None | Some(b'/')) {
            return true;
        }
        from = from + at + 1;
    }
    false
}

/// The processes of <processes> named one of <names> (BUILD_NAMES) placed in
/// <places>: by working
/// directory, else by an argument naming a checkout or its build root
/// (sccache's compilers run in the registry's source folder with
/// `--out-dir <checkout>/target/...`), else by the nearest ancestor placed
/// either way. sccache's own server belongs to no checkout; its compilers do.
pub fn build_processes(processes: &Processes, places: &[Place], names: &[&str]) -> Vec<BuildProcess> {
    let mut order: Vec<usize> = (0..places.len()).collect();
    // The longest path first: ~/dev/bae-worktrees/side-1 before ~/dev/bae.
    order.sort_by_key(|at| std::cmp::Reverse(places[*at].top.as_os_str().len()));
    let tops: Vec<(String, String)> = places
        .iter()
        .map(|place| (place.top.display().to_string(), place.root.display().to_string()))
        .collect();
    let mut cache: HashMap<i32, Option<usize>> = HashMap::new();
    let mut place_of = |pid: i32| -> Option<usize> {
        if let Some(known) = cache.get(&pid) {
            return *known;
        }
        let dir = cwd(pid).map(|dir| dir.display().to_string());
        let argv = args(pid).unwrap_or_default();
        let found = order.iter().copied().find(|at| {
            let (top, root) = &tops[*at];
            dir.as_deref().is_some_and(|dir| mentions(dir, top) || mentions(dir, root))
                || argv.iter().skip(1).any(|arg| mentions(arg, top) || mentions(arg, root))
        });
        cache.insert(pid, found);
        found
    };
    let mut found = Vec::new();
    for process in &processes.list {
        if !names.contains(&process.name.as_str()) {
            continue;
        }
        let Some(place) = processes.ancestry(process.pid).iter().take(12).find_map(|p| place_of(p.pid)) else {
            continue;
        };
        let (cpu_seconds, footprint) = usage(process.pid).unwrap_or((0.0, 0));
        found.push(BuildProcess {
            pid: process.pid,
            name: process.name.clone(),
            place,
            started: process.started,
            cpu_seconds,
            footprint,
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_child_process() {
        let dir = std::env::temp_dir();
        let mut child = std::process::Command::new("sleep").arg("30").current_dir(&dir).spawn().unwrap();
        let pid = child.id() as i32;
        let processes = Processes::read();
        let found = processes.get(pid).map(|p| (p.name.clone(), p.ppid));
        let place = cwd(pid);
        let argv = args(pid);
        let used = usage(pid);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(found, Some(("sleep".to_string(), std::process::id() as i32)));
        assert_eq!(place.map(|p| std::fs::canonicalize(p).unwrap()), Some(std::fs::canonicalize(&dir).unwrap()));
        assert_eq!(argv, Some(vec!["sleep".to_string(), "30".to_string()]));
        assert!(used.is_some_and(|(_, footprint)| footprint > 0));
        assert!(processes.ancestry(pid).iter().any(|p| p.pid == std::process::id() as i32));
    }

    #[test]
    fn places_a_build_process_by_its_folder_or_its_arguments() {
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("builds-place-{}", std::process::id()));
        let top = base.join("demo-worktrees/side-1");
        let other = base.join("demo");
        std::fs::create_dir_all(&top).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let places = vec![
            Place { label: "demo/main".into(), top: other.clone(), root: base.join("volume/demo/main") },
            Place { label: "demo/side-1".into(), top: top.clone(), root: base.join("volume/demo/side-1") },
        ];
        // A `sleep` in side-1's folder; a `sh` elsewhere naming main's build
        // root, as sccache's rustc names its --out-dir.
        let mut in_folder = std::process::Command::new("sleep").arg("30").current_dir(&top).spawn().unwrap();
        let out_dir = format!("{}/target/debug", places[0].root.display());
        let mut by_argument = std::process::Command::new("sh")
            .args(["-c", "sleep 30; :", "rustc", &out_dir])
            .current_dir("/")
            .spawn()
            .unwrap();
        let found = build_processes(&Processes::read(), &places, &["sleep", "sh"]);
        for child in [&mut in_folder, &mut by_argument] {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        std::fs::remove_dir_all(&base).unwrap();
        let placed = |pid: u32| found.iter().find(|p| p.pid == pid as i32).map(|p| places[p.place].label.clone());
        assert_eq!(placed(in_folder.id()), Some("demo/side-1".to_string()));
        assert_eq!(placed(by_argument.id()), Some("demo/main".to_string()));
    }

    #[test]
    fn lists_open_files() {
        let path = std::env::temp_dir().join(format!("builds-open-{}", std::process::id()));
        std::fs::write(&path, "x").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let open = open_files(std::process::id() as i32);
        drop(file);
        std::fs::remove_file(&path).unwrap();
        let wanted = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(path.file_name().unwrap());
        assert!(open.contains(&wanted), "{open:?}");
    }
}
