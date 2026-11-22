use super::*;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn a_land_command_names_its_worktree() {
    let line = argv(&["builds", "land", "/w/side-21", "--part", "7", "abc"]);
    assert_eq!(requested(&line), Some("/w/side-21"));
    assert_eq!(requested(&argv(&["builds", "land", "--help"])), None);
    assert_eq!(requested(&argv(&["builds", "app-build", "bae", "x"])), None);
}

#[test]
fn a_global_agent_option_before_land_does_not_hide_the_landing() {
    let line = argv(&[
        "builds",
        "--agent",
        "ab12",
        "land",
        "/w/side-20",
        "--part",
        "5",
        "abc",
    ]);
    assert_eq!(requested(&line), Some("/w/side-20"));
    let after = argv(&["builds", "land", ".", "--agent", "ab12", "--part", "5"]);
    assert_eq!(requested(&after), Some("."));
    assert_eq!(
        requested(&argv(&["builds", "--agent", "ab12", "land"])),
        None
    );
}

#[test]
fn a_landing_waits_for_the_other_process_that_holds_its_worktrees_turn() {
    let dir = std::env::temp_dir().join(format!("land-turns-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("land.lock");
    // Another landing holds the lock and wrote itself into the file.
    let mut file = crate::flock::open_creating(&path).unwrap();
    assert!(crate::flock::try_lock(&file, libc::LOCK_EX, &path).unwrap());
    std::io::Write::write_all(&mut file, b"process 36208 landing --done 1276 abc").unwrap();
    assert_eq!(
        held_by_another(&path, 99).unwrap().as_deref(),
        Some("process 36208 landing --done 1276 abc")
    );
    assert_eq!(held_by_another(&path, 36208).unwrap(), None);
    drop(file);
    assert_eq!(
        held_by_another(&path, 99).unwrap(),
        None,
        "a free lock holds nobody"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
