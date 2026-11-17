use super::*;

#[test]
fn a_missing_folder_on_this_mac_says_to_run_install_sh() {
    assert_eq!(
        missing_folder_lines(false, false, false),
        [
            "DerivedData  (no folder: run agent-instructions' install.sh)",
            "caches  (no folder: run agent-instructions' install.sh)",
        ]
    );
    assert!(missing_folder_lines(true, true, false).is_empty());
}

#[test]
fn a_missing_folder_on_a_host_says_what_a_host_keeps_instead() {
    let lines = missing_folder_lines(false, false, true);
    assert_eq!(lines.len(), 2);
    for line in &lines {
        assert!(
            !line.contains("run agent-instructions' install.sh"),
            "{line}"
        );
    }
    assert!(
        lines[1].starts_with("caches  (none on a host:"),
        "{}",
        lines[1]
    );
    assert!(lines[1].contains("sccache"), "{}", lines[1]);
}

#[test]
fn setup_here_marks_this_mac_as_a_host() {
    let scratch = crate::testing::Scratch::new("status-host");
    let places = scratch.places();
    assert!(!places.is_host().unwrap());
    places.mark_host().unwrap();
    assert!(places.is_host().unwrap());
}

#[test]
fn a_lists_folder_with_no_parent_has_no_place_for_the_host_marker() {
    let scratch = crate::testing::Scratch::new("status-host-root");
    let mut places = scratch.places();
    places.lists = std::path::PathBuf::from("/");
    let refusal = places.is_host().unwrap_err().text();
    assert!(refusal.contains("no parent folder"), "{refusal}");
    assert!(places.mark_host().is_err());
}
