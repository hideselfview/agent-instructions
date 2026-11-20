//! Keeping this Mac awake while builds works on another host over ssh.
//!
//! A Mac that sleeps answers nothing on its connections. A host that keeps
//! sending a command's output and hears nothing back ends the connection
//! within about twenty seconds, which stops the command there: the step or
//! job fails with ssh's "Connection reset by peer" once this Mac wakes. So
//! while builds drives work on a host (a lane's app checkout there,
//! app_remote.rs, or a job of `builds remote run` and `builds cargo`,
//! remote.rs), it holds two of macOS's power assertions, as IOKit's
//! IOPMLib.h describes them: PreventUserIdleSystemSleep keeps the Mac from
//! sleeping because nobody uses it, on battery as on power, and
//! NetworkClientActive, on power, also keeps it awake through a maintenance
//! wake (a dark wake), where the first has no effect. Neither keeps a Mac on
//! battery awake once its lid closes or through a maintenance wake; when such
//! a sleep ends a connection, the failure names it (sleeps.rs).
//!
//! Work on this Mac itself needs neither: a sleep only pauses it. macOS
//! releases a process's assertions when the process ends, however it ends.
//! A Linux machine is only ever a host, which drives no other host, so builds
//! holds nothing there.

use crate::fail::Result;
use crate::host::Host;

/// Holds this Mac awake until dropped.
#[derive(Debug)]
pub struct KeepAwake {
    #[cfg(target_os = "macos")]
    _held: Vec<os::Assertion>,
}

impl KeepAwake {
    /// Keeps this Mac awake while builds does <what> on <host>; for this
    /// Mac's own work it holds nothing. The assertions carry <what> and the
    /// host as their name, which `pmset -g assertions` shows.
    pub fn for_host(host: &Host, what: &str) -> Result<KeepAwake> {
        #[cfg(target_os = "macos")]
        {
            let mut held = Vec::new();
            if host.target.is_some() {
                let name = format!("builds: {what} on host {}", host.name);
                for kind in os::KINDS {
                    held.push(os::Assertion::take(kind, &name)?);
                }
            }
            Ok(KeepAwake { _held: held })
        }
        #[cfg(target_os = "linux")]
        {
            let _ = (host, what);
            Ok(KeepAwake {})
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::ffi::CString;

    use crate::core_foundation::{CFStringRef, CfString};
    use crate::fail;
    use crate::fail::Result;

    /// The assertions builds holds, as IOKit names their types.
    pub const KINDS: [&str; 2] = ["PreventUserIdleSystemSleep", "NetworkClientActive"];

    /// kIOPMAssertionLevelOn.
    const ON: u32 = 255;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    /// One power assertion this process holds; dropping it releases it.
    #[derive(Debug)]
    pub struct Assertion(u32);

    impl Assertion {
        /// Takes an assertion of <kind> named <name>; fails, naming the
        /// kind and IOKit's return code, when powerd refuses it.
        pub fn take(kind: &str, name: &str) -> Result<Assertion> {
            let kind_text = CfString::new(&CString::new(kind).expect("a kind holds no NUL"));
            let name_text = CfString::new(&CString::new(name).expect("a name holds no NUL"));
            let mut id = 0;
            let code = unsafe {
                IOPMAssertionCreateWithName(kind_text.as_ptr(), ON, name_text.as_ptr(), &mut id)
            };
            if code != 0 {
                fail!(
                    "macOS refused builds the power assertion {kind} ({name}): IOKit returned {code:#x}"
                );
            }
            Ok(Assertion(id))
        }
    }

    impl Drop for Assertion {
        fn drop(&mut self) {
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "awake_tests.rs"]
mod tests;
