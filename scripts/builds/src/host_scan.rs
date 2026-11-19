//! One reading of a host as a whole, for `builds health` and the dashboard:
//! its processes and how much of its CPU sits idle. One script prints them,
//! over the ssh builds already uses for hosts or with sh on this Mac, so a
//! health check asks each host once. stuck.rs judges the processes and
//! host_load.rs the idle share.

use crate::host::{Accept, Host, Ssh, Terminal, command_at, output_of, place};
use crate::stuck::{PS_FORMAT, Row, parse_ps};

/// The line after which the script prints its `ps` listing: every line
/// before it is `<key>=<value>`, and no `ps` line can equal it, since each
/// starts with a pid.
const PS_FOLLOWS: &str = "ps_follows=1";

/// What a host's script printed, read.
#[derive(Clone, Debug)]
pub struct Scan {
    pub rows: Vec<Row>,
    /// The share of the host's CPU that sat idle over one second, as a
    /// percentage, or why the script could not read it.
    pub cpu_idle: std::result::Result<f64, String>,
}

/// The script a host runs. `top` takes two samples a second apart, and the
/// second one says what the CPU did in that second; the first counts from
/// boot.
pub fn script(host: &Host) -> String {
    format!(
        r#"{root}echo "cpu=$(top -l 2 -n 0 -s 1 2>&1 | awk '/^CPU usage/{{line=$0}} END{{print line}}')"
echo {PS_FOLLOWS}
ps -axww -o {PS_FORMAT}"#,
        root = crate::remote::root_script(host),
    )
}

/// The idle share in top's line `CPU usage: 84.8% user, 15.1% sys, 0.0%
/// idle`.
fn parse_cpu(line: &str) -> std::result::Result<f64, String> {
    line.split(',')
        .find_map(|part| part.trim().strip_suffix("% idle")?.trim().parse().ok())
        .ok_or_else(|| format!("top printed no idle share: {line:?}"))
}

/// Reads what `script` printed.
pub fn parse(text: &str) -> std::result::Result<Scan, String> {
    let Some((head, ps)) = text.split_once(&format!("\n{PS_FOLLOWS}\n")) else {
        return Err(format!("the host printed no process listing: {text:?}"));
    };
    let value = |key: &str| crate::host::answer_value(head, key);
    Ok(Scan {
        rows: parse_ps(ps)?,
        cpu_idle: parse_cpu(value("cpu").unwrap_or_default()),
    })
}

/// Runs the script on <host> and reads what it printed.
pub fn read(ssh: &Ssh, host: &Host) -> std::result::Result<Scan, String> {
    let text = output_of(
        command_at(ssh, host, &script(host), Terminal::None),
        &format!("reading {}", place(&host.name)),
        Accept::Zero,
    )?;
    parse(&text)
}

#[cfg(test)]
#[path = "host_scan_tests.rs"]
mod tests;
