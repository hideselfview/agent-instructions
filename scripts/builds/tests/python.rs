//! A whole `builds` run: a command that runs steps puts a Python of 3.11 or
//! later first on its steps' PATH, whichever `python3` comes first on the
//! PATH it was given, including the commands that return before the rest of
//! the program starts.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

mod programs;

const BUILDS: &str = env!("CARGO_BIN_EXE_builds");

/// A script <name> in <folder> that says it is Python <major>.<minor>.
fn fake_python(folder: &Path, name: &str, minor: u32) -> std::path::PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let file = folder.join(name);
    std::fs::write(&file, format!("#!/bin/sh\necho 3 {minor}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    file
}

#[test]
fn a_step_run_by_programs_hold_finds_python_3_11_or_later_before_macos_3_9() {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("builds-python-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    // The step runs in a git checkout, as every step does.
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&dir)
            .status()
            .unwrap()
            .success()
    );
    programs::programs_folder(&home);
    fake_python(&dir.join("system"), "python3", 9);
    let newer = fake_python(&dir.join("brew"), "python3.13", 13);
    // A newer Python that cannot run is passed over, and builds says why.
    let broken = dir.join("brew/python3.14");
    std::fs::write(&broken, "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths([
        dir.join("system"),
        dir.join("brew"),
        "/bin".into(),
        "/usr/bin".into(),
    ])
    .unwrap();
    let ran = Command::new(BUILDS)
        .current_dir(&dir)
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("BUILDS_ROOT", dir.join("volume"))
        .env("BUILDS_POWER_RECORD", dir.join("power"))
        .env("BUILDS_LISTS", dir.join("lists"))
        .env("PATH", path)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("BUILDS_AGENT")
        .args([
            "programs",
            "hold",
            "--",
            "/bin/sh",
            "-c",
            r#"readlink "$(command -v python3)""#,
        ])
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success(), "{said}");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        newer.display().to_string(),
        "{said}"
    );
    assert!(
        said.contains(&format!("python: {} failed", broken.display())),
        "{said}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
