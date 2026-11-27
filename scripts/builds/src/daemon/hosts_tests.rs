use super::*;
use crate::host::tests::fake_ssh;
use crate::testing::Scratch;

#[test]
fn a_host_row_says_the_hosts_load_and_a_host_that_does_not_answer_says_why() {
    let scratch = Scratch::new("daemon-hosts");
    let places = scratch.places();
    std::fs::create_dir_all(&places.home).unwrap();
    let file = host::hosts_file(&places);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "m1 user@m1 10\ngone unreachable 4\n").unwrap();
    let ssh = fake_ssh(&scratch, &scratch.dir.join("remote-home"));

    let rows = read(&places, &ssh).unwrap();
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["m1", "gone"], "this Mac has its own card");
    assert!(rows[0].load.is_some() && rows[0].unreachable.is_none());
    assert!(rows[1].load.is_none());
    assert!(
        rows[1]
            .unreachable
            .as_deref()
            .is_some_and(|said| said.contains("Connection refused")),
        "{:?}",
        rows[1].unreachable
    );
}

#[test]
fn no_remote_host_means_no_rows() {
    let scratch = Scratch::new("daemon-hosts-empty");
    let places = scratch.places();
    let ssh = fake_ssh(&scratch, &scratch.dir.join("remote-home"));
    assert!(read(&places, &ssh).unwrap().is_empty());
}
