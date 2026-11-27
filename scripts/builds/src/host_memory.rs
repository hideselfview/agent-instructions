//! A host's memory pressure, which `builds health` reports next to the load:
//! a host can show a load inside its band while it swaps, and every build on
//! it then slows down. The probe script (host.rs) prints the values here: the
//! kernel's memory pressure level, which decides whether the host is over its
//! band, and the free share and swap in use, which `builds health` only
//! reports (macOS keeps swap allocated long after pressure ends).

use crate::fail;
use crate::fail::Result;

/// A host's memory pressure level, numbered 1, 2 and 4 as macOS's kernel
/// numbers its own (`kern.memorystatus_vm_pressure_level`); on Linux the
/// probe script judges it from the memory available (`SCRIPT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressure {
    Normal = 1,
    Warning = 2,
    Critical = 4,
}

impl Pressure {
    /// The level that the number <text> names; any other value fails and
    /// names it.
    fn parse(text: &str) -> Result<Pressure> {
        match text {
            "1" => Ok(Pressure::Normal),
            "2" => Ok(Pressure::Warning),
            "4" => Ok(Pressure::Critical),
            other => fail!(
                "the host's memory pressure level is {other:?}, not 1 (normal), 2 (warning) or 4 (critical)"
            ),
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Pressure::Normal => "normal",
            Pressure::Warning => "warning",
            Pressure::Critical => "critical",
        }
    }
}

/// What a host's memory is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Memory {
    /// The share of memory free, 0 to 100: the share `memory_pressure` calls
    /// free on macOS, and MemAvailable's share of MemTotal on Linux.
    pub free_percent: u32,
    /// The swap in use, in MiB.
    pub swap_used_mib: f64,
    pub pressure: Pressure,
}

/// Runs on the host (as part of the probe script, after the lines that set
/// `os`, host_platform.rs) to print the lines `Memory::parse` reads.
///
/// On macOS the kernel reports its own pressure level, `memory_pressure` the
/// free share, and awk turns the unit of `vm.swapusage`'s used amount (K, M
/// or G) into MiB. Linux reports no level of its own, so the script judges
/// one from /proc/meminfo's MemAvailable, the memory the kernel can give
/// without swapping: warning under 10% of MemTotal and critical under 5%,
/// with that share as the free share and SwapTotal less SwapFree as the swap
/// in use.
pub const SCRIPT: &str = r#"case "$os" in
Darwin)
  echo memory_free_percent=$(memory_pressure | awk '/free percentage/{gsub("%","",$NF); print $NF}')
  echo memory_pressure_level=$(sysctl -n kern.memorystatus_vm_pressure_level)
  echo swap_used_mib=$(sysctl -n vm.swapusage | awk '{for(i=1;i<=NF;i++) if($i=="used"){v=$(i+2); u=substr(v,length(v)); n=substr(v,1,length(v)-1)+0; if(u=="G")n*=1024; else if(u=="K")n/=1024; printf "%.2f", n}}')
  ;;
*)
  awk '/^MemTotal:/{t=$2} /^MemAvailable:/{a=$2} /^SwapTotal:/{st=$2} /^SwapFree:/{sf=$2} END{p=int(a*100/t); l=1; if(p<10)l=2; if(p<5)l=4; print "memory_free_percent=" p; print "memory_pressure_level=" l; printf "swap_used_mib=%.2f\n", (st-sf)/1024}' /proc/meminfo
  ;;
esac"#;

impl Memory {
    /// Reads the lines `SCRIPT` printed in <text>.
    pub fn parse(text: &str) -> Result<Memory> {
        let value = |key: &str| -> Result<&str> {
            match crate::host::answer_value(text, key) {
                Some(value) => Ok(value),
                None => fail!("the host's answer has no {key}: {text:?}"),
            }
        };
        let free = value("memory_free_percent")?;
        let Ok(free_percent) = free.parse::<u32>() else {
            fail!("the host's free memory is not a percentage: {free:?}");
        };
        let swap = value("swap_used_mib")?;
        let Ok(swap_used_mib) = swap.parse::<f64>() else {
            fail!("the host's swap in use is not a number of MiB: {swap:?}");
        };
        let level = value("memory_pressure_level")?;
        let pressure = Pressure::parse(level)?;
        Ok(Memory {
            free_percent,
            swap_used_mib,
            pressure,
        })
    }

    /// Whether the kernel reports memory pressure at warning or critical.
    /// `builds health` counts such a host as over its load band, whatever its
    /// load.
    pub fn under_pressure(&self) -> bool {
        self.pressure != Pressure::Normal
    }

    /// `memory pressure normal, 85% free, swap 0M used`.
    pub fn text(&self) -> String {
        format!(
            "memory pressure {}, {}% free, swap {:.0}M used",
            self.pressure.word(),
            self.free_percent,
            self.swap_used_mib
        )
    }

    /// A host whose memory is at rest, for tests.
    #[cfg(test)]
    pub fn at_rest() -> Memory {
        Memory {
            free_percent: 80,
            swap_used_mib: 0.0,
            pressure: Pressure::Normal,
        }
    }
}

#[cfg(test)]
#[path = "host_memory_tests.rs"]
mod tests;
