use super::*;
use crate::host::LOCAL;
use crate::testing::Scratch;

fn remote_host() -> Host {
    Host {
        band: None,
        name: "m1".to_string(),
        target: Some("u@m1".to_string()),
        cores: 10,
        speed: None,
    }
}

fn local_host() -> Host {
    Host {
        band: None,
        name: LOCAL.to_string(),
        target: None,
        cores: 10,
        speed: None,
    }
}

#[test]
fn a_run_that_starts_build_work_gets_a_folder_on_the_volume_that_its_end_deletes() {
    let scratch = Scratch::new("run-tmp-enter");
    let places = scratch.places();
    let held = enter(&places, "cargo", &["test".to_string()])
        .unwrap()
        .expect("cargo starts build work");
    let tmpdir = std::path::PathBuf::from(std::env::var_os("TMPDIR").unwrap());
    assert_eq!(tmpdir, held.folder);
    assert_eq!(tmpdir.parent().unwrap(), places.root.join(FOLDER));
    let id = tmpdir.file_name().unwrap().to_string_lossy().into_owned();
    assert!(id.starts_with(&format!("{}-", std::process::id())), "{id}");
    assert!(tmpdir.is_dir());
    drop(held);
    assert!(!tmpdir.exists());
}

#[test]
fn a_command_that_starts_no_build_work_gets_no_folder() {
    let scratch = Scratch::new("run-tmp-none");
    let places = scratch.places();
    for (command, args) in [
        ("health", vec![]),
        ("item", vec!["list".to_string()]),
        ("shots", vec!["findings".to_string()]),
        ("shots", vec!["check-program".to_string()]),
    ] {
        assert!(
            enter(&places, command, &args).unwrap().is_none(),
            "{command}"
        );
    }
    assert!(!places.root.join(FOLDER).exists());
    for (command, args) in [
        ("cargo", vec![]),
        ("app-build", vec![]),
        ("lock", vec![]),
        ("remote", vec![]),
        ("shots", vec!["run".to_string()]),
    ] {
        assert!(starts_work(command, &args), "{command}");
    }
}

#[test]
fn a_run_folder_can_have_a_symlink_and_a_path_through_its_parent() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new("run-tmp-classify");
    let root = scratch.places().root;
    let parent = root.join(FOLDER);
    let run = parent.join("1-2");
    std::fs::create_dir_all(&run).unwrap();
    let persistent = root.join(".sccache-tmp");
    std::fs::create_dir(&persistent).unwrap();
    let alias = root.join("alias");
    symlink(&run, &alias).unwrap();

    assert_eq!(classify(&root, &alias).unwrap(), Folder::Run);
    assert_eq!(
        classify(&root, &persistent.join("../.tmp/1-2")).unwrap(),
        Folder::Run
    );
    assert_eq!(
        classify(&root, &parent.join("../.sccache-tmp")).unwrap(),
        Folder::Directory
    );
    assert_eq!(classify(&root, &parent).unwrap(), Folder::Directory);
    std::fs::write(run.join("file"), "").unwrap();
    assert_eq!(
        classify(&root, &run.join("file")).unwrap(),
        Folder::NotDirectory
    );
    assert_eq!(classify(&root, &run.join("gone")).unwrap(), Folder::Missing);
}

#[test]
fn a_run_inside_a_run_makes_no_folder_of_its_own() {
    let scratch = Scratch::new("run-tmp-nested");
    let mut places = scratch.places();
    let outer = places.root.join(FOLDER).join("12-34");
    std::fs::create_dir_all(&outer).unwrap();
    places.tmpdir = Some(outer.clone());
    assert!(enter(&places, "cargo", &[]).unwrap().is_none());
    // A folder that its run already deleted names no run.
    std::fs::remove_dir_all(&outer).unwrap();
    assert!(enter(&places, "cargo", &[]).unwrap().is_some());
}

#[test]
fn a_run_with_no_build_volume_fails_instead_of_writing_to_the_system_temp_folder() {
    let scratch = Scratch::new("run-tmp-unmounted");
    let mut places = scratch.places();
    places.root = scratch.dir.join("not-mounted");
    let said = enter(&places, "cargo", &[]).unwrap_err().text();
    assert!(said.contains("is not mounted"), "{said}");
}

/// A run folder named for a process that ended, one for this process, and an
/// entry builds did not make.
fn folders(places: &Places) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let mut ended = std::process::Command::new("true").spawn().unwrap();
    let gone = ended.id();
    ended.wait().unwrap();
    let parent = places.root.join(FOLDER);
    let make = |name: String| {
        let folder = parent.join(name);
        std::fs::create_dir_all(folder.join("inside")).unwrap();
        folder
    };
    (
        make(format!("{gone}-1")),
        make(crate::remote::job_id()),
        make("not-a-run".to_string()),
    )
}

#[test]
fn the_sweep_deletes_the_folders_of_runs_that_ended_and_reports_what_no_run_made() {
    let scratch = Scratch::new("run-tmp-sweep");
    let places = scratch.places();
    let (finished, running, stray) = folders(&places);
    let swept = sweep(
        &places,
        &crate::host::Ssh::from_env(),
        &local_host(),
        &Holders::read().unwrap(),
    )
    .unwrap();
    assert_eq!(swept.deleted, [finished.display().to_string()]);
    assert_eq!(swept.unexpected, ["not-a-run"]);
    assert!(!finished.exists() && running.is_dir() && stray.is_dir());
}

#[test]
fn a_host_over_ssh_makes_its_folder_in_the_script_and_the_script_deletes_it() {
    let scratch = Scratch::new("run-tmp-script");
    let volume = scratch.dir.join("volume");
    std::fs::create_dir_all(&volume).unwrap();
    let script = format!(
        "BUILDS_ROOT={}; export BUILDS_ROOT\n{}printf '%s\\n' \"$TMPDIR\" \"$SCCACHE_START_SERVER\" \"$SCCACHE_ERROR_LOG\"\n{}",
        volume.display(),
        remote_setup(&remote_host(), "99-5"),
        remote_cleanup(&remote_host()),
    );
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .env_remove("TMPDIR")
        .env("SCCACHE_START_SERVER", "1")
        .env("SCCACHE_ERROR_LOG", volume.join("server.log"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let said = String::from_utf8(output.stdout).unwrap();
    let mut lines = said.lines();
    let tmpdir = lines.next().unwrap();
    assert_eq!(
        std::path::PathBuf::from(tmpdir),
        volume.join(FOLDER).join("99-5")
    );
    assert_eq!(lines.next(), Some("0"));
    let error_log = lines.next().unwrap();
    assert!(error_log.contains(crate::sccache_service::LABEL));
    assert!(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(error_log)
            .is_err()
    );
    assert!(!std::path::Path::new(tmpdir).exists());
}

#[test]
fn a_job_on_this_mac_gets_no_script_lines_and_keeps_the_tmpdir_it_inherits() {
    assert_eq!(remote_setup(&local_host(), "99-5"), "");
    assert_eq!(remote_cleanup(&local_host()), "");
}
