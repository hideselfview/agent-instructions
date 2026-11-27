//! The dates Finder shows for a file or folder as Date Created, Date
//! Modified and Date Added. std sets neither the first nor the last, so
//! macOS's setattrlist sets all three in one call.

use std::path::Path;
use std::time::SystemTime;

use crate::fail::Result;

/// Makes Finder show <at> as all three dates of <path> itself. The files
/// inside a folder keep theirs.
#[cfg(target_os = "macos")]
pub fn set(path: &Path, at: SystemTime) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    use crate::fail::Context;

    let name = std::ffi::CString::new(path.as_os_str().as_bytes())
        .context(|| format!("dating {}", path.display()))?;
    let since = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .context(|| format!("dating {}", path.display()))?;
    let time = libc::timespec {
        tv_sec: since.as_secs() as libc::time_t,
        tv_nsec: since.subsec_nanos() as libc::c_long,
    };
    // SAFETY: attrlist is plain integers, for which zero is a value.
    let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
    list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    list.commonattr = libc::ATTR_CMN_CRTIME | libc::ATTR_CMN_MODTIME | libc::ATTR_CMN_ADDEDTIME;
    // One value for each attribute, in the order of the attributes' bits.
    let mut values = [time; 3];
    // SAFETY: the name ends in NUL, and the list and the values outlive the
    // call, which is given the values' own size.
    let set = unsafe {
        libc::setattrlist(
            name.as_ptr(),
            (&raw mut list).cast(),
            (&raw mut values).cast(),
            size_of_val(&values),
            0,
        )
    };
    if set != 0 {
        return Err(std::io::Error::last_os_error())
            .context(|| format!("dating {}", path.display()));
    }
    Ok(())
}

/// Only a Mac has Finder, and only a Mac builds the app that gets the dates.
#[cfg(not(target_os = "macos"))]
pub fn set(path: &Path, _at: SystemTime) -> Result<()> {
    crate::fail!("dating {}: only macOS has Finder's dates", path.display())
}
