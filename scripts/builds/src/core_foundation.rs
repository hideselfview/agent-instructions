//! CoreFoundation's strings and release, declared here by hand, for the
//! macOS calls that take them: FSEvents' paths (daemon/fsevents_macos.rs)
//! and the power assertions that keep this Mac awake (awake.rs).

use std::ffi::{CStr, c_void};
use std::os::raw::c_char;

pub type CFStringRef = *const c_void;

const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        text: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    pub fn CFRelease(object: *const c_void);
}

/// A CFString this process made from UTF-8 text; dropping it releases it.
/// CoreFoundation makes none from text that is not UTF-8, and then the
/// string is null.
pub struct CfString(CFStringRef);

impl CfString {
    pub fn new(text: &CStr) -> CfString {
        CfString(unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) })
    }

    pub fn as_ptr(&self) -> CFStringRef {
        self.0
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}
