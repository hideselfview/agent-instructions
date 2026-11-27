use super::*;
use crate::testing::Scratch;

#[test]
fn the_daemon_prunes_at_once_and_then_every_interval_after_the_later_of_a_finish_and_its_start() {
    let hours = PRUNE_HOURS * 3600;
    assert_eq!(next_start(None, None), 0, "no prune ever ran");
    assert_eq!(next_start(Some(1000), None), 1000 + hours);
    assert_eq!(
        next_start(Some(1000), Some(2000)),
        2000 + hours,
        "a prune the daemon started failed"
    );
    assert_eq!(
        next_start(Some(3000), Some(2000)),
        3000 + hours,
        "a prune finished after the daemon's start"
    );
}

#[test]
fn a_prune_that_deletes_records_its_finish_and_a_dry_run_records_nothing() {
    let scratch = Scratch::new("prune-record");
    let tool = scratch.tool(0);
    crate::prune::cmd_prune(
        &tool,
        &crate::args::Args::of(crate::command::Command::Prune, &["--dry-run".to_string()]).unwrap(),
    )
    .unwrap();
    assert_eq!(last_finished(&tool.places).unwrap(), None);
    let before = crate::app_timing::now();
    crate::prune::cmd_prune(
        &tool,
        &crate::args::Args::of(crate::command::Command::Prune, &[]).unwrap(),
    )
    .unwrap();
    let finished = last_finished(&tool.places).unwrap().expect("recorded");
    assert!(finished >= before, "{finished} < {before}");
}

#[test]
fn a_prune_fails_at_once_while_another_runs() {
    let scratch = Scratch::new("prune-lock");
    let tool = scratch.tool(0);
    let running = hold(&tool.places).unwrap();
    let refused = crate::prune::cmd_prune(
        &tool,
        &crate::args::Args::of(crate::command::Command::Prune, &[]).unwrap(),
    )
    .expect_err("refused")
    .text();
    assert!(
        refused.contains("another builds prune is running"),
        "{refused}"
    );
    assert_eq!(last_finished(&tool.places).unwrap(), None);
    drop(running);
}

#[test]
fn a_record_that_holds_no_time_is_an_error_that_names_it() {
    let scratch = Scratch::new("prune-damaged");
    let places = scratch.places();
    std::fs::create_dir_all(places.daemon_dir()).unwrap();
    std::fs::write(places.prune_record(), "soon\n").unwrap();
    let error = last_finished(&places).expect_err("refused").text();
    assert!(error.contains("pruned holds \"soon\\n\""), "{error}");
    let (line, advice) = health(&places, 0);
    assert!(line.contains("pruned holds"), "{line}");
    assert!(advice.is_some(), "a damaged record is no finding");
}

#[test]
fn health_lists_prune_when_none_finished_in_twice_the_interval() {
    let scratch = Scratch::new("prune-health");
    let places = scratch.places();
    let (line, advice) = health(&places, 1_800_000_000);
    assert!(line.contains("no prune has finished"), "{line}");
    assert!(
        advice.is_some(),
        "no prune ever finished, and health is quiet"
    );

    record_finished(&places, 1_800_000_000).unwrap();
    let (line, advice) = health(&places, 1_800_000_000 + 60);
    assert!(line.contains("(1m ago)"), "{line}");
    assert_eq!(advice, None);
    let (_, advice) = health(&places, 1_800_000_000 + 2 * PRUNE_HOURS * 3600 + 60);
    assert!(
        advice.is_some(),
        "a prune finished long ago, and health is quiet"
    );
}

#[test]
fn the_daemon_runs_prune_and_writes_what_it_says_to_the_prune_log() {
    let scratch = Scratch::new("prune-run");
    let places = scratch.places();
    std::fs::create_dir_all(places.daemon_dir()).unwrap();
    let exe = scratch.dir.join("builds");
    std::fs::write(
        &exe,
        "#!/bin/bash\necho \"ran $1\"\necho \"said on stderr\" >&2\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    run_once(&places, &exe);
    let said = std::fs::read_to_string(places.prune_log()).unwrap();
    assert_eq!(said, "ran prune\nsaid on stderr\n");
}
