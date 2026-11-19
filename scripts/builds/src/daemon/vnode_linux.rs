//! Linux's side of vnode.rs. The watch is a kqueue's, which Linux lacks, and
//! the dashboard, the one program that watches folders, runs only on a Mac
//! (`cmd_dashboard`), so every watch fails to start here.

use std::io;
use std::path::Path;

/// A running watch; none ever runs off macOS.
pub enum Watch {}

impl Watch {
    pub fn start(_folder: &Path, _changed: impl Fn() + Send + 'static) -> io::Result<Watch> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a folder watch uses macOS's kqueue, and this machine runs Linux",
        ))
    }
}
