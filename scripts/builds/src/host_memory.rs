//! A host's memory pressure, which `builds health` reports next to the load:
//! a host can show a load inside its band while it swaps, and every build on
//! it then slows down. The probe script (host.rs) prints the values here: the
//! kernel's memory pressure level, which decides whether the host is over its
//! band, and the free share and swap in use, which `builds health` only
//! reports (macOS keeps swap allocated long after pressure ends).

use crate::fail;
use crate::fail::Result;

/// The kernel's memory pressure level (`kern.memorystatus_vm_pressure_level`),
/// which the kernel numbers 1, 2 and 4.
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
    /// The share of memory `memory_pressure` calls free, 0 to 100.
    pub free_percent: u32,
    /// The swap in use, in MiB.
    pub swap_used_mib: f64,
    pub pressure: Pressure,
}

/// Runs on the host (as part of the probe script) to print the lines
/// `Memory::parse` reads. awk turns the unit of `vm.swapusage`'s used amount
/// (K, M or G) into MiB.
pub const SCRIPT: &str = r#"echo memory_free_percent=$(memory_pressure | awk '/free percentage/{gsub("%","",$NF); print $NF}')
echo memory_pressure_level=$(sysctl -n kern.memorystatus_vm_pressure_level)
echo swap_used_mib=$(sysctl -n vm.swapusage | awk '{for(i=1;i<=NF;i++) if($i=="used"){v=$(i+2); u=substr(v,length(v)); n=substr(v,1,length(v)-1)+0; if(u=="G")n*=1024; else if(u=="K")n/=1024; printf "%.2f", n}}')"#;

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

/// This Mac's memory, read by the same script a remote host runs.
pub fn here() -> Result<Memory> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(SCRIPT)
        .stdin(std::process::Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            Memory::parse(&String::from_utf8_lossy(&output.stdout))
        }
        Ok(output) => fail!(
            "reading this Mac's memory failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => fail!("running sh to read this Mac's memory: {error}"),
    }
}

#[cfg(test)]
#[path = "host_memory_tests.rs"]
mod tests;
