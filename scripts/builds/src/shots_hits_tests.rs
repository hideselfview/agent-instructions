use super::*;

const NOW: i64 = 10_000_000;

fn row(
    lane: &str,
    environment: &str,
    ago: i64,
    base_run: BaseRun,
    pictures: Option<(u32, u32)>,
) -> Row {
    Row {
        repo: "bae".to_string(),
        lane: lane.to_string(),
        environment: environment.to_string(),
        at: NOW - ago,
        base_run,
        pictures,
    }
}

#[test]
fn each_window_counts_the_saved_and_built_base_runs_and_the_screenshots_it_covers() {
    let rows = [
        row("mac", "E1", 60, BaseRun::Saved, Some((10, 0))),
        row("mac", "E1", 7_200, BaseRun::Built, Some((0, 10))),
        row("mac", "E1", 3 * DAY, BaseRun::Saved, None),
        row("mac", "E1", 60, BaseRun::NotNeeded, Some((4, 0))),
        row("android", "E2", 60, BaseRun::Built, Some((0, 3))),
    ];
    assert_eq!(
        lines(&rows, NOW),
        [
            "shots cache of bae's android lane in environment E2: base runs saved when a build looked for them: hour 0% of 1, day 0% of 1, week 0% of 1; screenshots taken from the picture folder: hour 0% of 3, day 0% of 3, week 0% of 3",
            "shots cache of bae's mac lane in environment E1: base runs saved when a build looked for them: hour 100% of 1, day 50% of 2, week 67% of 3; screenshots taken from the picture folder: hour 100% of 14, day 58% of 24, week 58% of 24",
        ]
    );
}

#[test]
fn a_window_with_no_lookup_says_none() {
    let rows = [row("mac", "E1", 60, BaseRun::NotNeeded, None)];
    assert!(lines(&rows, NOW)[0].contains("hour none, day none, week none"));
}

#[test]
fn a_lane_whose_builds_rarely_found_the_saved_run_is_listed() {
    let miss = |ago| row("mac", "E1", ago, BaseRun::Built, None);
    let rows: Vec<Row> = (0..5).map(|i| miss(60 * (i + 1))).collect();
    let found = findings(&rows, NOW);
    assert_eq!(found.len(), 1);
    assert!(
        found[0]
            .text
            .contains("for 0 of the 5 comparisons of bae's mac lane in environment E1"),
        "{}",
        found[0].text
    );
}

#[test]
fn few_comparisons_a_good_rate_or_old_misses_list_nothing() {
    let miss = |ago| row("mac", "E1", ago, BaseRun::Built, None);
    let hit = |ago| row("mac", "E1", ago, BaseRun::Saved, None);
    // Four comparisons in a day are too few to judge.
    assert!(findings(&(0..4).map(|i| miss(60 * (i + 1))).collect::<Vec<_>>(), NOW).is_empty());
    // Two of ten saved is the lowest share that is left alone.
    let mut rows: Vec<Row> = (0..8).map(|i| miss(60 * (i + 1))).collect();
    rows.extend((0..2).map(|i| hit(60 * (i + 1))));
    assert!(findings(&rows, NOW).is_empty());
    // Misses from over a day ago count for the week, not the day.
    assert!(findings(&(0..9).map(|_| miss(2 * DAY)).collect::<Vec<_>>(), NOW).is_empty());
}
