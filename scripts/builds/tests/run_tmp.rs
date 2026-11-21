//! A child of `builds cargo` sees a TMPDIR on the build volume, which the end
//! of the run deletes, on a scratch volume with a stand-in for cargo that
//! writes down its environment. The same holds for a shots step of an app
//! build (app_queue.rs, `a_shots_step_sees_a_tmpdir_on_the_build_volume`).

use std::path::PathBuf;
use std::process::Command;

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

#[test]
fn a_child_of_builds_cargo_sees_a_tmpdir_on_the_build_volume_that_the_end_of_the_run_deletes() {
    let dir = PathBuf::from(format!("/private/tmp/rt-cargo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (volume, bin, work) = (dir.join("volume"), dir.join("bin"), dir.join("work"));
    for folder in [
        &volume,
        &bin,
        &work,
        &dir.join("home"),
        &dir.join("system-tmp"),
    ] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let cargo = bin.join("cargo");
    std::fs::write(
        &cargo,
        "#!/bin/sh\necho \"$TMPDIR\" > \"$RECORD\"\nfor made in tmpdir-file; do echo x > \"$TMPDIR/$made\"; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&cargo, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let record = dir.join("record");
    let ran = Command::new(BUILDS)
        .args(["cargo", "check"])
        .current_dir(&work)
        .env("BUILDS_ROOT", &volume)
        .env("HOME", dir.join("home"))
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("TMPDIR", dir.join("system-tmp"))
        .env("RECORD", &record)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&ran.stderr).into_owned();
    assert!(ran.status.success(), "{said}");
    let seen = PathBuf::from(std::fs::read_to_string(&record).unwrap().trim());
    assert_eq!(seen.parent().unwrap(), volume.join(".tmp"), "{said}");
    assert!(!seen.exists(), "the run's folder is still there: {said}");
    assert_eq!(
        std::fs::read_dir(dir.join("system-tmp")).unwrap().count(),
        0,
        "the run wrote to the system's temporary folder"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
