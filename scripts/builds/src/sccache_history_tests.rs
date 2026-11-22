use super::*;

const NOW: i64 = 1_795_069_061;

fn sample(host: &str, server: i32, ago: i64, hits: u64, misses: u64) -> Reading {
    Reading {
        host: host.to_string(),
        server,
        at: NOW - ago,
        executed: hits + misses,
        rust_hits: hits,
        rust_misses: misses,
    }
}

/// A host gets a sample every five minutes, not each minute, and samples
/// older than a week and an hour go.
#[test]
fn a_host_gets_a_sample_every_five_minutes_and_the_last_week_stays() {
    let mut samples = vec![
        sample("m1", 7, KEEP_SECONDS + 10, 1, 1),
        sample("m1", 7, 200, 5, 5),
    ];
    assert!(!record(&mut samples, sample("m1", 7, 0, 6, 6)));
    assert!(record(&mut samples, sample("local", 7, 0, 6, 6)));
    assert!(record(&mut samples, sample("m1", 7, -100, 7, 7)));
    assert_eq!(
        samples.iter().map(|s| s.rust_hits).collect::<Vec<_>>(),
        [5, 6, 7],
        "the sample older than a week went"
    );
}

/// The rate over a window starts from the newest sample at or before the
/// window's start and adds the growth of each sample after it.
#[test]
fn the_rate_is_the_growth_since_the_sample_at_the_window_start() {
    let samples = vec![
        sample("m1", 7, 7200, 100, 10),
        sample("m1", 7, 3600, 200, 20),
        sample("m1", 7, 1800, 250, 30),
        sample("m1", 7, 0, 300, 40),
        sample("local", 7, 0, 5, 5),
    ];
    assert_eq!(
        rate(&samples, "m1", NOW, HOUR),
        Some(Rate {
            since: Some(NOW - 3600),
            hits: 100,
            misses: 20,
        })
    );
    assert_eq!(
        rate(&samples, "m1", NOW, DAY),
        Some(Rate {
            since: Some(NOW - 7200),
            hits: 200,
            misses: 30,
        }),
        "with no sample a day old, the rate covers the time since the oldest"
    );
}

/// A server that started again counts from zero: the sample after its start
/// adds all of its counts, never a negative growth.
#[test]
fn a_sample_across_a_server_restart_starts_from_zero() {
    let samples = vec![
        sample("m1", 7, 3600, 1000, 100),
        sample("m1", 7, 1800, 1100, 110),
        sample("m1", 8, 900, 30, 20),
        sample("m1", 8, 0, 80, 25),
    ];
    assert_eq!(
        rate(&samples, "m1", NOW, HOUR),
        Some(Rate {
            since: Some(NOW - 3600),
            hits: 100 + 30 + 50,
            misses: 10 + 20 + 5,
        })
    );
    let same_pid = vec![sample("m1", 7, 1800, 1100, 110), sample("m1", 7, 0, 30, 20)];
    assert_eq!(
        rate(&same_pid, "m1", NOW, HOUR),
        Some(Rate {
            since: Some(NOW - 1800),
            hits: 30,
            misses: 20,
        }),
        "counts that went down belong to a server that started again"
    );
}

/// A host with fewer than two samples in reach has no rate.
#[test]
fn a_host_with_one_sample_has_no_rate() {
    let samples = vec![sample("m1", 7, 0, 5, 5)];
    assert_eq!(rate(&samples, "m1", NOW, HOUR), None);
    assert_eq!(rate(&samples, "local", NOW, HOUR), None);
    assert_eq!(rate(&[], "m1", NOW, HOUR), None);
}

/// The text names each window's share and count, and says where the history
/// is shorter than the window.
#[test]
fn the_text_gives_each_windows_share_and_when_it_covers_less() {
    let samples = vec![
        sample("m1", 7, 1800, 100, 100),
        sample("m1", 7, 0, 190, 110),
    ];
    let text = rates_text(&samples, "m1", NOW);
    let since = crate::text::local_minute(NOW - 1800);
    assert_eq!(
        text,
        format!(
            "hit rate of Rust compiles: hour 90% of 100 (since {since}), day 90% of 100 (since {since}), week 90% of 100 (since {since})"
        )
    );
    assert_eq!(
        rates_text(&[], "m1", NOW),
        "hit rate of Rust compiles: no history yet"
    );
}

/// A window with no Rust compile says so instead of a share.
#[test]
fn a_window_with_no_compile_has_no_share() {
    let samples = vec![
        sample("m1", 7, 3600, 100, 100),
        sample("m1", 7, 0, 100, 100),
    ];
    assert_eq!(
        rates_text(&samples, "m1", NOW),
        format!(
            "hit rate of Rust compiles: hour no compile, day no compile (since {0}), week no compile (since {0})",
            crate::text::local_minute(NOW - 3600)
        )
    );
}
