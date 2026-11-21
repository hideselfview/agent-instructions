use super::*;
use crate::host_scan::CacheServer;

const NOW: i64 = 1_795_069_061;

fn stats(hits: u64, misses: u64, size: u64) -> CacheStats {
    CacheStats {
        executed: hits + misses,
        rust_hits: hits,
        rust_misses: misses,
        cache_size: size,
        max_cache_size: 53_687_091_200,
    }
}

fn reading(host: &str, server: i32, minutes_ago: i64, hits: u64, misses: u64) -> Reading {
    Reading {
        host: host.to_string(),
        server,
        at: NOW - minutes_ago * 60,
        executed: hits + misses,
        rust_hits: hits,
        rust_misses: misses,
    }
}

/// The rate covers the time since the newest reading of the same server at
/// least five minutes old; a server that started again, or with no such
/// reading, is counted since it started.
#[test]
fn the_rate_covers_the_last_minutes_of_the_same_server() {
    let readings = vec![
        reading("m1", 7, 20, 100, 100),
        reading("m1", 7, 6, 300, 300),
        reading("m1", 7, 2, 390, 390),
        reading("local", 7, 10, 0, 0),
    ];
    let now = stats(400, 700, 10);
    assert_eq!(
        rate(&readings, "m1", 7, &now, NOW),
        Rate {
            since: Some(NOW - 6 * 60),
            hits: 100,
            misses: 400,
        }
    );
    let since_start = Rate {
        since: None,
        hits: 400,
        misses: 700,
    };
    assert_eq!(rate(&readings, "m1", 8, &now, NOW), since_start);
    assert_eq!(rate(&readings[2..], "m1", 7, &now, NOW), since_start);
    assert_eq!(
        rate(&readings, "m1", 7, &stats(10, 10, 10), NOW).since,
        None,
        "counts that went down belong to a server that started again"
    );
    assert_eq!(
        Rate {
            since: Some(NOW - 6 * 60),
            hits: 100,
            misses: 400,
        }
        .text()
        .split(" since ")
        .next(),
        Some("20% of 500 Rust compiles")
    );
}

fn m1() -> Host {
    Host {
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: Some(0.8),
        band: None,
    }
}

/// m1 with 10 cores, whose server (pid 44790) runs <running> compiles while
/// <clients> clients name a compiler, and whose cache holds <size> bytes of
/// 50 GiB.
fn m1_with_server(
    running: usize,
    clients: usize,
    size: u64,
) -> (Host, std::result::Result<Scan, String>) {
    let mut ps =
        String::from("44790 1 502 0.0 10:00.00 2-07:02:40 S /Users/u/.cargo/bin/sccache\n");
    for at in 0..running {
        ps += &format!(
            "{} 44790 502 90.0 0:10.00 00:10 S /Users/u/.rustup/toolchains/x/bin/rustc --crate-name c{at}\n",
            50_000 + at
        );
    }
    for at in 0..clients {
        ps += &format!(
            "{} 6000 502 0.0 0:00.01 02:00 S sccache /Users/u/.rustup/toolchains/x/bin/rustc --crate-name c{at}\n",
            60_000 + at
        );
    }
    (
        m1(),
        Ok(Scan {
            uid: 502,
            at: 0,
            rows: crate::stuck::parse_ps(&ps).unwrap(),
            cpu_idle: Ok(0.0),
            jobs: Vec::new(),
            sccache: Ok(Some(CacheServer {
                pid: 44790,
                stats: stats(35361, 34774, size),
            })),
            front_app: Ok(None),
            screen_locked: Ok(None),
        }),
    )
}

/// A server whose waiting clients number more than twice the host's cores
/// is a finding that says so with the cache's rate and, when the cache is
/// full, that it is; every host gets a line with what its server does.
#[test]
fn a_host_whose_clients_wait_more_than_twice_its_cores_has_too_much_compile_work() {
    let mut readings = vec![reading("m1", 44790, 10, 35000, 34000)];
    let (caches, changed) = judge_with(
        &[m1_with_server(10, 119, 53_670_801_410)],
        &mut readings,
        NOW,
    );
    assert!(changed);
    assert_eq!(readings.len(), 2);
    let since = local_minute(NOW - 10 * 60);
    assert_eq!(
        lines(&caches),
        [format!(
            "compile cache on host m1 (sccache pid 44790): 10 compiles run, 109 clients wait; it answered 32% of 1135 Rust compiles since {since}; cache 50.0G of 50.0G"
        )]
    );
    assert_eq!(
        findings(&caches)
            .iter()
            .map(Advice::line)
            .collect::<Vec<_>>(),
        [format!(
            "sccache: host m1's server has 109 clients waiting while it runs 10 compiles, more than 2 for each of the host's 10 cores, so the host has more compile work than it can run; its cache answered 32% of 1135 Rust compiles since {since}; its cache is full (50.0G of 50.0G), so each new entry pushes an old one out; start nothing new for host m1"
        )]
    );
    let (calm, _) = judge_with(&[m1_with_server(10, 30, 10 << 30)], &mut Vec::new(), NOW);
    assert!(findings(&calm).is_empty());
}

/// A host with no server, or one whose server the scan could not read, gets
/// a line that says so and no finding.
#[test]
fn a_host_with_no_server_or_an_unread_one_gets_a_line() {
    let (host, scan) = m1_with_server(0, 0, 0);
    let mut none = scan.clone().unwrap();
    none.sccache = Ok(None);
    let mut unread = scan.unwrap();
    unread.sccache = Err("sccache printed no stats".to_string());
    let (caches, _) = judge_with(
        &[(host.clone(), Ok(none)), (host, Ok(unread))],
        &mut Vec::new(),
        NOW,
    );
    assert_eq!(
        lines(&caches),
        [
            "compile cache on host m1: no sccache server listens",
            "compile cache on host m1: not read: sccache printed no stats",
        ]
    );
    assert!(findings(&caches).is_empty());
}
