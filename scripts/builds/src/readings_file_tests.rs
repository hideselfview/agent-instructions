use super::*;

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
                for at in 0..100_i64 {
                    write(&file, &[at]).unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    assert_eq!(read::<i64>(&file).unwrap().len(), 1);
}
