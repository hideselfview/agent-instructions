use super::*;
use crate::lists::fixtures::make_store;
use crate::testing::Scratch;

fn sample(host: &str, load_1: f64) -> Sample {
    Sample {
        host: host.to_string(),
        cores: 10,
        load_1,
        memory: Pressure::Normal,
    }
}

fn rows(db: &Connection) -> Vec<(String, i64, f64, String)> {
    db.prepare("SELECT host, at, load_1, memory FROM host_sample ORDER BY at, host")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

#[test]
fn only_the_hosts_that_answered_have_a_sample() {
    let probe = Probe {
        platform: crate::host_platform::Platform::here(),
        cores: 8,
        load: [3.0, 2.0, 1.0],
        free_kib: 0,
        memory: crate::host_memory::Memory {
            free_percent: 40,
            swap_used_mib: 0.0,
            pressure: Pressure::Warning,
        },
        git_fails: None,
        nextest: crate::cargo_program::Has::Nothing,
        mutants: crate::cargo_program::Has::Nothing,
        node: crate::cargo_program::Has::Nothing,
        processes: crate::proc_limit::ProcessUse {
            count: 100,
            limit: 10_000,
        },
    };
    let host = |name: &str| Host {
        name: name.to_string(),
        target: None,
        cores: 8,
        speed: None,
        band: None,
    };
    let answers = vec![
        (host("m1"), Ok(probe)),
        (host("off"), Err("no route".to_string())),
    ];
    assert_eq!(
        samples_of(&answers),
        vec![Sample {
            host: "m1".into(),
            cores: 8,
            load_1: 3.0,
            memory: Pressure::Warning,
        }]
    );
}

#[test]
fn a_write_keeps_the_last_thirty_days_and_deletes_older_samples() {
    let db = crate::db::memory();
    let mut db = db;
    let now = 1_790_000_000;
    record(&mut db, now - 31 * 86400, &[sample("local", 1.0)]).unwrap();
    record(&mut db, now - 29 * 86400, &[sample("local", 2.0)]).unwrap();
    record(&mut db, now, &[sample("local", 3.0), sample("m1", 4.0)]).unwrap();
    assert_eq!(
        rows(&db),
        vec![
            (
                "local".to_string(),
                now - 29 * 86400,
                2.0,
                "normal".to_string()
            ),
            ("local".to_string(), now, 3.0, "normal".to_string()),
            ("m1".to_string(), now, 4.0, "normal".to_string()),
        ]
    );
}

#[test]
fn two_samples_of_one_host_at_one_second_are_refused_and_nothing_is_written() {
    let mut db = crate::db::memory();
    assert!(
        record(
            &mut db,
            1_790_000_000,
            &[sample("local", 1.0), sample("local", 2.0)]
        )
        .is_err()
    );
    assert!(rows(&db).is_empty());
}

#[test]
fn a_sample_of_this_mac_goes_to_every_stores_table() {
    let scratch = Scratch::new("host-sample-once");
    let places = scratch.places();
    std::fs::create_dir_all(&places.home).unwrap();
    let first = make_store(&places, &["bae"]);
    let second = crate::lists::fixtures::make_store_named(&places, "other", &["coven"]);
    sample_once(&places, &crate::testing::no_ssh(), 1_790_000_000).unwrap();
    for store in [first, second] {
        let db = store.open_existing().unwrap();
        let found = rows(&db);
        assert_eq!(found.len(), 1, "{}: {found:?}", store.project);
        assert_eq!(found[0].0, "local");
        assert_eq!(found[0].1, 1_790_000_000);
    }
}
