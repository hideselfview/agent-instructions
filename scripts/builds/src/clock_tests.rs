use super::*;

/// When the machine booted, in seconds since 1970, as the system records it:
/// macOS's kern.boottime, Linux's btime line in /proc/stat.
fn boot_epoch() -> i64 {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sysctl")
            .args(["-n", "kern.boottime"])
            .output()
            .expect("sysctl runs");
        // `{ sec = 1794752000, usec = 123456 } Fri Nov 14 ...`
        let text = String::from_utf8_lossy(&output.stdout);
        let seconds = text
            .split("sec = ")
            .nth(1)
            .and_then(|rest| rest.split(',').next())
            .expect("kern.boottime names its seconds");
        seconds.trim().parse().expect("the seconds are a number")
    }
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/stat").expect("/proc/stat reads");
        stat.lines()
            .find_map(|line| line.strip_prefix("btime "))
            .expect("/proc/stat names btime")
            .trim()
            .parse()
            .expect("btime is a number")
    }
}

#[test]
fn the_clock_counts_every_second_since_boot_sleep_included() {
    // A clock that stops in sleep falls behind the wall clock's time since
    // boot by every second the machine has slept since then.
    let wall = crate::app_timing::now() - boot_epoch();
    let reading = since_boot().as_secs() as i64;
    assert!(
        (reading - wall).abs() <= 5,
        "the clock reads {reading}s since boot, the wall clock {wall}s"
    );
}

#[test]
fn a_moment_measures_the_time_since_it() {
    let moment = Moment::now();
    std::thread::sleep(Duration::from_millis(50));
    let elapsed = moment.elapsed();
    assert!(elapsed >= Duration::from_millis(50), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}
