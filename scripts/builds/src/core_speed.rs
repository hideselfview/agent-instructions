//! How fast one core of a Mac compiles, against this Mac's: the factor
//! `builds host setup` records for each host and `host::rank_by_room` weighs
//! a host's free cores by. This module times one fixed compile, rustc on one
//! core over the same generated source, on the host and here, and keeps the
//! fastest of `TRIES` on each, since a busy Mac only ever slows a compile.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::fail;
use crate::fail::{Context, Result};

/// How many times each Mac runs the compile.
const TRIES: usize = 3;

/// The source the compile reads: generic functions that rustc type-checks,
/// monomorphizes and optimizes, about a second's work on one core.
fn source() -> String {
    let mut text = String::new();
    for at in 0..80 {
        let step = at % 7 + 2;
        text.push_str(&format!(
            "pub fn f{at}<T: Copy + core::ops::Add<Output = T> + core::ops::Mul<Output = T>>(xs: &[T], k: T) -> Option<T> {{
    let mut acc: Option<T> = None;
    for (n, x) in xs.iter().enumerate() {{
        let v = if n % {step} == 0 {{ *x * k }} else {{ *x + k }};
        acc = Some(match acc {{ Some(a) => a + v, None => v }});
    }}
    acc
}}
pub fn g{at}(xs: &[u64]) -> u64 {{
    f{at}(xs, {at}u64).unwrap_or(0)
        + f{at}(&xs.iter().map(|x| *x as f64).collect::<Vec<_>>(), 1.5).map_or(0, |v| v as u64)
}}
"
        ));
    }
    text
}

/// The fastest of `TRIES` runs of the fixed compile on this Mac, in folder
/// <dir>, which it creates and removes.
pub fn time_compile(dir: &Path) -> Result<Duration> {
    std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
    let timed = (|| -> Result<Duration> {
        let file = dir.join("speed.rs");
        std::fs::write(&file, source()).context(|| format!("writing {}", file.display()))?;
        let mut fastest = Duration::MAX;
        for _ in 0..TRIES {
            let started = Instant::now();
            let output = Command::new("rustc")
                .args(["--edition", "2021", "--crate-type", "lib"])
                .args(["-C", "opt-level=2", "-C", "codegen-units=1"])
                .args(["--emit=obj", "-o"])
                .arg(dir.join("speed.o"))
                .arg(&file)
                .stdin(Stdio::null())
                .output()
                .context(|| "running rustc".to_string())?;
            if !output.status.success() {
                fail!(
                    "the speed compile failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            fastest = fastest.min(started.elapsed());
        }
        Ok(fastest)
    })();
    std::fs::remove_dir_all(dir).context(|| format!("removing {}", dir.display()))?;
    timed
}

/// What `builds host speed-here` prints for <took>.
pub fn line(took: Duration) -> String {
    format!("ms={}", took.as_millis())
}

/// Reads what `line` printed.
pub fn parse_line(text: &str) -> Result<Duration> {
    let Some(ms) = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("ms="))
    else {
        fail!("the host did not say how long its speed compile took: {text:?}");
    };
    match ms.parse::<u64>() {
        Ok(ms) if ms > 0 => Ok(Duration::from_millis(ms)),
        _ => fail!("the host's speed compile time {ms:?} is not a positive number of ms"),
    }
}

/// A host's core speed against this Mac's: this Mac's time over the host's.
pub fn factor(here: Duration, host: Duration) -> f64 {
    here.as_secs_f64() / host.as_secs_f64()
}

#[cfg(test)]
#[path = "core_speed_tests.rs"]
mod tests;
