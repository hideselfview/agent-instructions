//! The copies of the claude.ai canvas runtime (`artifact-type/dc-runtime.js`
//! of a canvas) that boards are drawn with, kept by sha256 in
//! `scripts/boards/runtime/<sha256>.js`. Each repo that publishes a canvas
//! records the sha256 of the runtime its canvas was last published with in
//! `design/mocks/runtime.toml`, and its boards are drawn with that copy; a
//! repo that publishes no canvas has its boards drawn with the newest copy.
//!
//! To add a copy: read `artifact-type/dc-runtime.js` of a canvas with the
//! Artifact tool (action read), save it as `runtime/<its sha256>.js`, and add
//! its entry to `COPIES` with the version and the time of that read.

/// One copy of the canvas runtime and where it was read.
#[derive(Debug, PartialEq, Eq)]
pub struct RuntimeCopy {
    /// The sha256 of `text`, as hex.
    pub sha256: &'static str,
    /// The canvas's version when the copy was read.
    pub version: &'static str,
    /// The canvas the copy was read from.
    pub source: &'static str,
    /// When the copy was read, as RFC 3339.
    pub read_at: &'static str,
    pub text: &'static str,
}

/// Every copy, oldest first.
pub const COPIES: &[RuntimeCopy] = &[RuntimeCopy {
    sha256: "b795ee21261f2e7da1b10cb25b4a4cfa275d1b96165f89396f1fed426e191844",
    version: "1794441585-926d",
    source: "https://claude.ai/artifact/9geFwGqEzevbvbh22NqZzJ",
    read_at: "2026-11-12T00:01:28Z",
    text: include_str!(
        "../runtime/b795ee21261f2e7da1b10cb25b4a4cfa275d1b96165f89396f1fed426e191844.js"
    ),
}];

/// The copy whose sha256 is <sha256>, if there is one.
pub fn copy(sha256: &str) -> Option<&'static RuntimeCopy> {
    COPIES.iter().find(|copy| copy.sha256 == sha256)
}

/// The copy read last.
pub fn newest() -> &'static RuntimeCopy {
    COPIES
        .iter()
        .max_by_key(|copy| copy.read_at)
        .expect("there is a runtime copy")
}

/// The file in agent-instructions that holds the copy with <sha256>.
pub fn copy_file(sha256: &str) -> String {
    format!("scripts/boards/runtime/{sha256}.js")
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
