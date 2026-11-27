//! A host's load band: the 5-minute load average `builds health` aims to keep
//! the host in. Health says to start agents while the load is under the low
//! end, and to start nothing new while it is over the high end. `builds host
//! band` sets a host's band, and the file `hosts` keeps it as a line
//! `band <name> <low> <high>`, for this Mac (`local`) as for a recorded host.
//! A host with no band keeps the shares of its cores below.

use crate::fail;
use crate::fail::Result;

/// With no band, a host has idle cores under this share of its cores.
const IDLE_SHARE: f64 = 0.75;
/// With no band, a host is over its cores above this share of them.
const OVER_SHARE: f64 = 1.25;

/// The word that starts a band line in the hosts file. No host may take it as
/// its name.
pub const LINE_WORD: &str = "band";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub low: f64,
    pub high: f64,
}

impl Band {
    /// The band of a host with <cores> cores and no band of its own.
    pub fn of_cores(cores: u32) -> Band {
        let cores = f64::from(cores);
        Band {
            low: IDLE_SHARE * cores,
            high: OVER_SHARE * cores,
        }
    }

    /// The band <low> to <high>, as typed or read from the hosts file.
    pub fn parse(low: &str, high: &str) -> Result<Band> {
        let number = |text: &str| match text.parse::<f64>() {
            Ok(number) if number > 0.0 && number.is_finite() => Ok(number),
            _ => Err(format!("{text:?} is not a positive number")),
        };
        let (low, high) = match (number(low), number(high)) {
            (Ok(low), Ok(high)) => (low, high),
            (Err(said), _) | (_, Err(said)) => fail!("a load band needs two numbers: {said}"),
        };
        if low >= high {
            fail!("a load band's low end ({low}) must be under its high end ({high})");
        }
        Ok(Band { low, high })
    }

    /// `25 to 30`.
    pub fn text(&self) -> String {
        format!("{} to {}", self.low, self.high)
    }

    /// The hosts-file line that sets this band on host <name>.
    pub fn line(&self, name: &str) -> String {
        format!("{LINE_WORD} {name} {} {}\n", self.low, self.high)
    }
}

/// `builds host band <name> <low> <high>` and `builds host band <name> clear`.
pub fn cmd_band(tool: &crate::tool::Tool, args: &crate::args::Args) -> Result<()> {
    let (name, band) = match args.positional() {
        [name, _clear] => (name, None),
        [name, low, high] => (name, Some(Band::parse(low, high)?)),
        _ => fail!(
            "{}",
            crate::command::Command::Host(crate::command::host::Band).usage_text()
        ),
    };
    crate::host::set_band(&tool.places, name, band)?;
    match band {
        Some(band) => crate::output::say(format!(
            "{} has a 5-minute load band of {}",
            crate::host::place(name),
            band.text()
        )),
        None => crate::output::say(format!(
            "{} has no load band; health judges it by the shares of its cores",
            crate::host::place(name)
        )),
    }
    Ok(())
}
