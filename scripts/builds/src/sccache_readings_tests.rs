use super::*;

const NOW: i64 = 1_795_069_061;

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

/// A host gets a reading at most once a minute, and readings over an hour
/// old go.
#[test]
fn a_reading_joins_once_a_minute_and_the_last_hour_stays() {
    let mut readings = vec![reading("m1", 7, 90, 1, 1), reading("m1", 7, 0, 5, 5)];
    assert!(!record(&mut readings, reading("m1", 7, 0, 6, 6)));
    assert_eq!(readings.len(), 2);
    assert!(record(&mut readings, reading("local", 7, 0, 6, 6)));
    assert_eq!(
        readings
            .iter()
            .map(|r| (r.host.as_str(), r.rust_hits))
            .collect::<Vec<_>>(),
        [("m1", 5), ("local", 6)]
    );
}

/// The reading compared with is the newest of the same host and server at
/// least five minutes old.
#[test]
fn compares_with_the_newest_reading_of_the_server_five_minutes_old() {
    let readings = vec![
        reading("m1", 7, 20, 1, 1),
        reading("m1", 7, 6, 2, 2),
        reading("m1", 7, 2, 3, 3),
        reading("m1", 8, 5, 4, 4),
        reading("local", 7, 10, 5, 5),
    ];
    assert_eq!(compared(&readings, "m1", 7, NOW), Some(&readings[1]));
    assert_eq!(compared(&readings, "m1", 8, NOW), Some(&readings[3]));
    assert_eq!(compared(&readings, "m1", 9, NOW), None);
    assert_eq!(compared(&readings, "local", 7, NOW), Some(&readings[4]));
}

/// Writers in one process never share a staged file: each write of the
/// readings completes whatever the others do.
#[test]
fn writes_of_the_readings_in_one_process_all_complete() {
    let scratch = crate::testing::Scratch::new("readings-writers");
    let file = scratch.dir.join("readings.json");
    let writers: Vec<_> = (0..8)
        .map(|_| {
            let file = file.clone();
            std::thread::spawn(move || {
                for at in 0..100 {
                    write_file(&file, &[reading("local", 7, at, 1, 1)]).unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
}
