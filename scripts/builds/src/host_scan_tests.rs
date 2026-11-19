use super::*;

const PS: &str = "    1     0     0   0.0   1:00.00 9-00:00:00 /sbin/launchd\n44790     1   501   0.0  10:00.00 2-07:02:40 /Users/x/.cargo/bin/sccache\n";

#[test]
fn reads_what_a_host_printed() {
    let scan = parse(&format!(
        "cpu=CPU usage: 95.17% user, 4.9% sys, 0.72% idle \n{PS_FOLLOWS}\n{PS}"
    ))
    .unwrap();
    assert_eq!(scan.cpu_idle, Ok(0.72));
    assert_eq!(scan.rows.len(), 2);
}

/// A top that printed no idle share leaves the rest of the reading whole;
/// a reading with no process listing is an error.
#[test]
fn a_missing_idle_share_is_an_error_of_its_own() {
    let scan = parse(&format!(
        "cpu=CPU usage: 95.17% user, 4.9% sys\n{PS_FOLLOWS}\n{PS}"
    ))
    .unwrap();
    assert!(scan.cpu_idle.unwrap_err().contains("no idle share"));
    assert_eq!(scan.rows.len(), 2);
    assert!(parse("cpu=\n").is_err());
}

/// The script, run on this Mac with a stand-in for top, reads top's second
/// sample, which covers the last second, and lists the processes.
#[test]
fn the_script_reads_the_last_second_of_the_cpu() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = crate::testing::Scratch::new("host-scan");
    let bin = scratch.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let top = bin.join("top");
    std::fs::write(
        &top,
        "#!/bin/sh\necho 'CPU usage: 10.0% user, 5.0% sys, 85.0% idle'\necho 'CPU usage: 50.0% user, 10.0% sys, 40.0% idle'\n",
    )
    .unwrap();
    std::fs::set_permissions(&top, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = std::process::Command::new("sh")
        .args(["-c", &script(&crate::host::Host::local())])
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();
    let scan = parse(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(scan.cpu_idle, Ok(40.0));
    assert!(
        scan.rows
            .iter()
            .any(|row| row.pid == std::process::id() as i32)
    );
}
