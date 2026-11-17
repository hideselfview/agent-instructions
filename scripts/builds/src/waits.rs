//! Where agents wait, by kind: each commit hook run, test run, app build
//! queue wait and app build step, with how long it took and the load of the
//! machine it ran on. `builds waits` prints the time of each kind per local
//! day and per band of load per core, with counts, medians and totals, and
//! the dashboard shows the same for today and yesterday. Each owner reads its
//! own rows (hook_stats.rs, cargo_run.rs, app_timing.rs); this module groups
//! them and stores nothing.
//!
//! Every wait counts, whatever its result: an agent waits as long for a hook
//! that fails as for one that passes.

use std::path::Path;
use std::time::Duration;

use crate::fail;
use crate::fail::Result;
use crate::hook_stats::median_of;
use crate::host::Load;

/// The kinds of work an agent waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A pre-commit hook run (`builds checks run`), from its start to its
    /// last check's end.
    Hook,
    /// A `builds cargo test` or `builds cargo nextest` run that no hook check
    /// started.
    TestRun,
    /// An app build run's wait in its lane's queue.
    AppBuildQueue,
    /// A step of an app build run, its test lines and shots runs included.
    AppBuildStep,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Hook => "commit hook",
            Kind::TestRun => "test run",
            Kind::AppBuildQueue => "app build queue",
            Kind::AppBuildStep => "app build step",
        }
    }
}

/// One wait: what kind, when it started, how long it took, and the cores and
/// one-minute load of the machine it ran on, when builds recorded them.
#[derive(Clone, Debug, PartialEq)]
pub struct Wait {
    pub kind: Kind,
    /// Seconds since 1970.
    pub started_at: i64,
    pub ms: u64,
    pub load: Option<Load>,
}

/// A band of load per core: the one-minute load average over the cores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    Under1,
    From1To2,
    From2To4,
    From4,
    /// The wait's record holds no load.
    NotRecorded,
}

impl Band {
    pub fn word(self) -> &'static str {
        match self {
            Band::Under1 => "under 1",
            Band::From1To2 => "1 to 2",
            Band::From2To4 => "2 to 4",
            Band::From4 => "4 and over",
            Band::NotRecorded => "not recorded",
        }
    }
}

/// The band <load> falls in.
pub fn band_of(load: Option<Load>) -> Band {
    let Some(load) = load else {
        return Band::NotRecorded;
    };
    match load.one_minute / f64::from(load.cores) {
        per_core if per_core < 1.0 => Band::Under1,
        per_core if per_core < 2.0 => Band::From1To2,
        per_core if per_core < 4.0 => Band::From2To4,
        _ => Band::From4,
    }
}

/// Every wait in the database at <path> that started at or after <since>
/// (seconds since 1970); empty when builds never recorded anything there.
pub fn read(path: &Path, since: i64) -> Result<Vec<Wait>> {
    let Some(conn) = crate::db::open_reader(path)? else {
        return Ok(Vec::new());
    };
    let mut waits = crate::hook_stats::waits(&conn, since, path)?;
    waits.extend(crate::cargo_run::test_waits(&conn, since, path)?);
    waits.extend(crate::app_timing::waits(&conn, since, path)?);
    Ok(waits)
}

/// The figures of one kind on one day, for all loads (<band> None) or for
/// one band.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: Kind,
    pub day: String,
    pub band: Option<Band>,
    pub count: u32,
    pub median_ms: u64,
    pub total_ms: u64,
}

/// The rows of <waits>: for each kind, each day (as <day_of> names the day
/// of a start), a row for all loads, then a row for each band that has a
/// wait. Kinds come in `Kind`'s order, days in the order of their names and
/// bands in `Band`'s order.
pub fn rows(waits: Vec<Wait>, day_of: &dyn Fn(i64) -> String) -> Vec<Row> {
    let mut groups: std::collections::BTreeMap<(Kind, String, Option<Band>), Vec<u64>> =
        std::collections::BTreeMap::new();
    for wait in waits {
        let day = day_of(wait.started_at);
        groups
            .entry((wait.kind, day.clone(), None))
            .or_default()
            .push(wait.ms);
        groups
            .entry((wait.kind, day, Some(band_of(wait.load))))
            .or_default()
            .push(wait.ms);
    }
    groups
        .into_iter()
        .map(|((kind, day, band), mut times)| Row {
            kind,
            day,
            band,
            count: times.len() as u32,
            total_ms: times.iter().sum(),
            median_ms: median_of(&mut times),
        })
        .collect()
}

/// <ms> as a person reads a wait: `45s`, `12m 26s`, `2h 05m`.
pub fn span(ms: u64) -> String {
    let total = Duration::from_millis(ms).as_secs_f64().round() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// The words of a row's band: `all loads` for the row of every load.
fn band_text(band: Option<Band>) -> &'static str {
    band.map_or("all loads", Band::word)
}

/// The text `builds waits` prints: <rows> of <project>, read since <since>
/// (a day, for the heading).
pub fn report(project: &str, since: Option<&str>, rows: &[Row]) -> String {
    let heading = match since {
        Some(day) => format!("project {project}, since {day}"),
        None => format!("project {project}, all days"),
    };
    if rows.is_empty() {
        return format!("{heading}: builds recorded no wait\n");
    }
    let width = rows
        .iter()
        .map(|row| row.kind.word().len())
        .max()
        .unwrap_or(0);
    let mut out = format!(
        "{heading}: the time agents waited on each kind of work, per day and per load per core\n\n{:<width$}  {:<10}  {:<13}  {:>5}  {:>7}  {:>7}\n",
        "kind", "day", "load per core", "count", "median", "total"
    );
    for row in rows {
        out.push_str(&format!(
            "{:<width$}  {:<10}  {:<13}  {:>5}  {:>7}  {:>7}\n",
            row.kind.word(),
            row.day,
            band_text(row.band),
            row.count,
            span(row.median_ms),
            span(row.total_ms)
        ));
    }
    out
}

/// Local midnight at the start of the day before the day of <now>. A day with
/// a clock change is not 24 hours long, so the function finds yesterday's own
/// midnight rather than subtracting 24 hours from today's.
fn start_of_yesterday(now: i64) -> Result<i64> {
    let today = crate::text::local_day(now);
    let Some(midnight) = crate::text::local_start_of(&today) else {
        fail!("today, {today}, has no local midnight");
    };
    let yesterday = crate::text::local_day(midnight - 1);
    match crate::text::local_start_of(&yesterday) {
        Some(start) => Ok(start),
        None => fail!("yesterday, {yesterday}, has no local midnight"),
    }
}

/// The dashboard's JSON for the project store at <path> as of <now>: the
/// rows of the waits since the start of yesterday, or `{"error"}` when builds
/// cannot read the database.
pub fn dashboard_json(path: &Path, now: i64) -> serde_json::Value {
    match start_of_yesterday(now).and_then(|since| read(path, since)) {
        Err(error) => serde_json::json!({ "error": error.text() }),
        Ok(waits) => serde_json::json!({
            "rows": rows(waits, &crate::text::local_day).iter().map(|row| serde_json::json!({
                "kind": row.kind.word(),
                "day": row.day,
                "band": band_text(row.band),
                "count": row.count,
                "median_ms": row.median_ms,
                "total_ms": row.total_ms,
            })).collect::<Vec<_>>(),
        }),
    }
}

const USAGE: &str = "usage: builds waits [--project <project>] [--since <YYYY-MM-DD>]";

/// `builds waits [--project P] [--since DATE]`.
pub fn cmd_waits(args: &[String]) -> Result<()> {
    let args = crate::lists::args::Args::parse(args, &["since"], &[])?;
    if !args.positional().is_empty() {
        fail!("{USAGE}");
    }
    let since = args.value("since")?;
    let start = crate::text::since_start(since)?.unwrap_or(0);
    let places = crate::places::Places::from_env()?;
    let store = crate::lists::store::Store::resolve(&places, &args, Path::new("."))?;
    let waits = read(&store.dir.join(crate::db::DATABASE), start)?;
    print!(
        "{}",
        report(&store.project, since, &rows(waits, &crate::text::local_day))
    );
    Ok(())
}

#[cfg(test)]
#[path = "waits_tests.rs"]
mod tests;
