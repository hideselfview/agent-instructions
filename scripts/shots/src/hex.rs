//! Digests as lowercase hex, the form the runtime copies and the
//! pinned Chrome builds name them in.

/// <digest>'s bytes as lowercase hex.
pub(crate) fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
