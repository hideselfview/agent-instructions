//! Linux's side of fsevents.rs. FSEvents is macOS's, and the dashboard, the
//! one program that follows the build volume, runs only on a Mac
//! (`cmd_dashboard`), so every stream fails to open here.

use std::path::Path;

use super::Handler;

/// The id of the latest event the system has recorded: Linux records none.
pub fn current_event_id() -> u64 {
    0
}

/// Linux keeps no event store for a device.
pub fn device_keeps_events(_device: u64) -> bool {
    false
}

/// A stream FSEvents made; none is ever made off macOS.
pub enum Raw {}

impl Raw {
    pub fn open(
        _since: u64,
        _folders: &[&Path],
        _excluded: &[&Path],
        _latency: f64,
        _handler: Handler,
    ) -> Result<Raw, String> {
        Err("FSEvents is macOS's, and this machine runs Linux".to_string())
    }
}
