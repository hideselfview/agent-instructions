//! macOS's side of procs.rs: the machine's processes, read through libproc
//! and sysctl.

use std::ffi::{CStr, OsString};
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::Process;
use crate::fail;
use crate::fail::Result;

fn c_text(bytes: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = bytes
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The pid of every process on the machine.
fn all_pids() -> Result<Vec<i32>> {
    pids_listed(|buffer| {
        let bytes = std::mem::size_of_val(buffer) as libc::c_int;
        let at = if buffer.is_empty() {
            std::ptr::null_mut()
        } else {
            buffer.as_mut_ptr().cast()
        };
        unsafe { libc::proc_listallpids(at, bytes) }
    })
}

/// The pids <list> writes, as `proc_listallpids` writes them: into the
/// buffer it is given, returning how many it wrote, or with an empty buffer
/// how many there are, or -1 when it fails. The kernel fills as much of the
/// buffer as fits, so a list that fills the whole buffer may be cut short:
/// the buffer grows until the list leaves room to spare.
fn pids_listed(mut list: impl FnMut(&mut [i32]) -> libc::c_int) -> Result<Vec<i32>> {
    let listed = |filled: libc::c_int| {
        if filled < 0 {
            fail!(
                "listing this Mac's processes failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(filled as usize)
    };
    let mut room = listed(list(&mut []))? + 256;
    loop {
        if libc::c_int::try_from(room * std::mem::size_of::<i32>()).is_err() {
            fail!("listing this Mac's processes failed: {room} pids do not fit in one list");
        }
        let mut pids = vec![0i32; room];
        let filled = listed(list(&mut pids))?;
        if filled < room {
            pids.truncate(filled);
            return Ok(pids);
        }
        room *= 2;
    }
}
/// Every process this user can read. Fails when the kernel would not list
/// the processes, so no caller takes a short list for the whole machine.
pub fn list() -> Result<Vec<Process>> {
    let pids = all_pids()?;
    let mut list = Vec::with_capacity(pids.len());
    for pid in pids {
        if pid == 0 {
            continue;
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
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
            name: if name.is_empty() {
                c_text(&info.pbi_comm)
            } else {
                name
            },
            started: info.pbi_start_tvsec as i64,
        });
    }
    Ok(list)
}

/// The process state of a zombie in `proc_bsdinfo.pbi_status` (SZOMB in
/// <sys/proc.h>): one that exited and waits for its parent to reap it.
const SZOMB: u32 = 5;

/// Has <pid> exited, as a zombie waiting for its parent or gone altogether?
/// The kernel keeps a zombie's pid, so `runs` says it runs, but it has no
/// arguments or working directory left to read.
pub fn ended(pid: i32) -> bool {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (read != size && !super::runs(pid)) || (read == size && info.pbi_status == SZOMB)
}

/// Registers before sending SIGTERM so an exit during the signal is recorded.
/// The kernel reports exits even for processes this caller did not start.
/// Fails if the kernel does not report the exit within ten seconds.
pub fn terminate_and_wait(pid: i32) -> io::Result<()> {
    use crate::kqueue::{Queue, event};
    let queue = Queue::new()?;
    if let Err(error) = queue.change(&[event(
        pid as libc::uintptr_t,
        libc::EVFILT_PROC,
        libc::EV_ADD | libc::EV_ONESHOT,
        libc::NOTE_EXIT,
    )]) {
        return super::accept_gone(error);
    }
    super::terminate(pid)?;
    let mut notification = [event(0, 0, 0, 0)];
    queue.wait(
        &mut notification,
        Some(Instant::now() + Duration::from_secs(10)),
    )?;
    Ok(())
}

/// The working directory of <pid>; None when it exited or can't be read.
pub fn cwd(pid: i32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    let path = path.to_string_lossy();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path.as_ref()))
    }
}

/// The KERN_PROCARGS2 buffer of <pid>: None when it exited or can't be read.
/// The kernel hides the environment of Apple's own programs (`/bin/zsh`,
/// `/bin/sleep`), so their buffers hold no environment.
fn procargs(pid: i32) -> Option<Vec<u8>> {
    let mut size: libc::size_t = 0;
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut argmax: libc::c_int = 0;
    let mut argmax_size = std::mem::size_of::<libc::c_int>();
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut argmax as *mut libc::c_int).cast(),
            &mut argmax_size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    let mut buffer = vec![0u8; argmax as usize];
    size = size.max(buffer.len());
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    buffer.truncate(size);
    Some(buffer)
}

/// The arguments and the environment (`NAME=value` strings) in a
/// KERN_PROCARGS2 <buffer>: the argument count, the executable's path, NULs,
/// the arguments, then the environment, each string ended by a NUL.
fn split_procargs(buffer: &[u8]) -> Option<(Vec<String>, Vec<OsString>)> {
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
    let mut parts = rest.split(|b| *b == 0);
    let text = |part: &[u8]| String::from_utf8_lossy(part).into_owned();
    let args = parts.by_ref().take(argc).map(text).collect();
    let env = parts
        .filter(|part| !part.is_empty())
        .map(|part| OsString::from_vec(part.to_vec()))
        .collect();
    Some((args, env))
}

/// The arguments <pid> was started with (KERN_PROCARGS2); None when it exited
/// or can't be read.
pub fn args(pid: i32) -> Option<Vec<String>> {
    split_procargs(&procargs(pid)?).map(|(args, _)| args)
}

/// The environment <pid> was started with, as `NAME=value` strings; None when
/// it exited or can't be read.
pub fn environment(pid: i32) -> Option<Vec<OsString>> {
    split_procargs(&procargs(pid)?).map(|(_, env)| env)
}

/// CPU time used so far, in seconds, and the physical footprint in bytes.
pub fn usage(pid: i32) -> Option<(f64, u64)> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut info as *mut libc::rusage_info_v2).cast(),
        )
    };
    if rc != 0 {
        return None;
    }
    let ticks = info.ri_user_time + info.ri_system_time;
    Some((ticks as f64 * timebase() / 1e9, info.ri_phys_footprint))
}

/// CPU time <pid> and every child it has waited for have used, in seconds:
/// a finished child's time counts in its parent's, so the sum over a live
/// tree is all the CPU the tree has used.
pub fn tree_cpu(pid: i32) -> Option<f64> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut info as *mut libc::rusage_info_v2).cast(),
        )
    };
    if rc != 0 {
        return None;
    }
    let ticks = info.ri_user_time
        + info.ri_system_time
        + info.ri_child_user_time
        + info.ri_child_system_time;
    Some(ticks as f64 * timebase() / 1e9)
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
        if info.denom == 0 {
            1.0
        } else {
            info.numer as f64 / info.denom as f64
        }
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
    let needed =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let slots = needed as usize / std::mem::size_of::<libc::proc_fdinfo>() + 16;
    let mut fds: Vec<libc::proc_fdinfo> = (0..slots)
        .map(|_| libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0,
        })
        .collect();
    let bytes = (fds.len() * std::mem::size_of::<libc::proc_fdinfo>()) as libc::c_int;
    let filled = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            bytes,
        )
    };
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
            libc::proc_pidfdinfo(
                pid,
                fd.proc_fd,
                PROC_PIDFDVNODEPATHINFO,
                (&mut info as *mut VnodeFdInfoWithPath).cast(),
                size,
            )
        };
        if read != size {
            continue;
        }
        let path = unsafe { CStr::from_ptr(info.pvip.vip_path.as_ptr().cast()) };
        paths.push(PathBuf::from(path.to_string_lossy().as_ref()));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_procargs_buffer_splits_into_arguments_and_environment() {
        let mut buffer = 2i32.to_ne_bytes().to_vec();
        buffer.extend(b"/bin/zsh\0\0\0zsh\0-c\0A=1\0BUILDS_AGENT=a7\0\0\0");
        let (args, env) = split_procargs(&buffer).unwrap();
        assert_eq!(args, ["zsh", "-c"]);
        assert_eq!(env, ["A=1", "BUILDS_AGENT=a7"]);
        assert!(split_procargs(&[1, 0]).is_none());
    }

    /// A list that fills the whole buffer may be cut short, so the buffer
    /// grows until the list fits with room to spare; a list that fails, at
    /// the count or at the read, fails the read.
    #[test]
    fn the_pid_buffer_grows_until_the_list_fits_and_a_failed_list_fails() {
        let every: Vec<i32> = (1..=1000).collect();
        let mut sizes = Vec::new();
        let listed = pids_listed(|buffer| {
            if buffer.is_empty() {
                return 10;
            }
            sizes.push(buffer.len());
            let n = buffer.len().min(every.len());
            buffer[..n].copy_from_slice(&every[..n]);
            n as libc::c_int
        })
        .unwrap();
        assert_eq!(listed, every);
        assert_eq!(sizes, [266, 532, 1064]);
        assert!(pids_listed(|_| -1).is_err());
        assert!(pids_listed(|buffer| if buffer.is_empty() { 10 } else { -1 }).is_err());
    }
}
