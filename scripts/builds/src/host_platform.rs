//! What a host runs: its OS and its processor, as `uname -s` and `uname -m`
//! name them there. Every probe asks (host.rs), so builds sends a host only
//! the work its OS can do: app builds and the settings UI tests need go only
//! to Macs, a build of a tool copies only between hosts of one platform, and
//! a cargo job that needs macOS stays on a Mac (cargo_place.rs).

use crate::fail;
use crate::fail::Result;

/// A host's OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
}

impl Os {
    /// The OS that `uname -s` names: Darwin or Linux. builds runs on no other.
    pub fn parse(uname: &str) -> Result<Os> {
        match uname {
            "Darwin" => Ok(Os::Mac),
            "Linux" => Ok(Os::Linux),
            other => fail!("the host runs {other:?}, and builds runs only on macOS and Linux"),
        }
    }

    /// The OS this program runs on.
    pub fn here() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }

    /// `macOS` or `Linux`.
    pub fn word(self) -> &'static str {
        match self {
            Os::Mac => "macOS",
            Os::Linux => "Linux",
        }
    }
}

/// A host's OS and processor. A program built for one platform runs on every
/// host of that platform and on no other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    /// The processor as `uname -m` names it: arm64 on an Apple silicon Mac,
    /// aarch64 or x86_64 on Linux.
    pub arch: String,
}

impl Platform {
    /// The platform this program runs on, named as `uname -m` names it here.
    pub fn here() -> Platform {
        let arch = match (Os::here(), std::env::consts::ARCH) {
            (Os::Mac, "aarch64") => "arm64",
            (_, arch) => arch,
        };
        Platform {
            os: Os::here(),
            arch: arch.to_string(),
        }
    }

    /// `Linux x86_64`.
    pub fn text(&self) -> String {
        format!("{} {}", self.os.word(), self.arch)
    }

    /// Reads the lines `SCRIPT` printed in <text>.
    pub fn parse(text: &str) -> Result<Platform> {
        let value = |key: &str| match crate::host::answer_value(text, key) {
            Some(value) if !value.is_empty() => Ok(value),
            _ => fail!("the host's answer has no {key}: {text:?}"),
        };
        Ok(Platform {
            os: Os::parse(value("os")?)?,
            arch: value("arch")?.to_string(),
        })
    }
}

/// Prints the lines `Platform::parse` reads, and sets `os` to what `uname -s`
/// says, for the lines after it to branch on.
pub const SCRIPT: &str = "os=$(uname -s)\necho \"os=$os\"\necho \"arch=$(uname -m)\"";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hosts_answer_names_its_platform_and_an_unknown_os_fails() {
        assert_eq!(
            Platform::parse("os=Linux\narch=x86_64\n").unwrap(),
            Platform {
                os: Os::Linux,
                arch: "x86_64".to_string()
            }
        );
        assert!(Platform::parse("os=FreeBSD\narch=amd64\n").is_err());
        assert!(Platform::parse("os=Darwin\n").is_err());
    }

    #[test]
    fn this_mac_names_its_platform_as_uname_does() {
        let output = std::process::Command::new("sh")
            .args(["-c", SCRIPT])
            .output()
            .unwrap();
        assert_eq!(
            Platform::parse(&String::from_utf8_lossy(&output.stdout)).unwrap(),
            Platform::here()
        );
    }
}
