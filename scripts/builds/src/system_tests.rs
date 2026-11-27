use super::*;
use crate::testing::Scratch;

/// Under a stand-in folder, a name of the table resolves to the folder's
/// program, and builds refuses a name the folder lacks, naming it and the
/// folder.
#[test]
fn a_stand_in_folder_gives_its_programs_and_refuses_the_rest() {
    let scratch = Scratch::new("system-stand-in");
    let folder = scratch.dir.join("system");
    std::fs::create_dir_all(&folder).unwrap();
    scratch.script("system/ioreg", "echo stand-in ioreg");
    let system = System::StandIn(folder.clone());
    assert_eq!(system.path(Program::Ioreg).unwrap(), folder.join("ioreg"));
    let refused = system.path(Program::Launchctl).unwrap_err().text();
    assert_eq!(
        refused,
        format!(
            "BUILDS_SYSTEM names the stand-in folder {}, which has no launchctl, so builds runs none",
            folder.display()
        )
    );
    assert!(system.command(Program::Launchctl).is_err());
}

/// A script under a stand-in folder runs the folder's program for each name
/// of the table, and a name the folder lacks fails there, naming it, with
/// nothing run.
#[test]
fn a_script_runs_the_stand_in_folders_programs() {
    let scratch = Scratch::new("system-script");
    let folder = scratch.dir.join("system");
    std::fs::create_dir_all(&folder).unwrap();
    let ioreg = scratch.script("system/ioreg", "echo \"stand-in ioreg $*\"");
    let system = System::StandIn(folder.clone());
    let said = system
        .sh("ioreg -n Root -d1; launchctl bootstrap gui/1 job; echo \"launchctl exit $?\"")
        .output()
        .unwrap();
    assert!(said.status.success());
    assert_eq!(
        String::from_utf8_lossy(&said.stdout),
        "stand-in ioreg -n Root -d1\nlaunchctl exit 127\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&said.stderr).trim(),
        format!(
            "BUILDS_SYSTEM names the stand-in folder {}, which has no launchctl, so builds runs none",
            folder.display()
        )
    );
    assert_eq!(system.path(Program::Ioreg).unwrap(), ioreg);
}

/// The machine's own programs run by name, and a script for a host runs as
/// it is written.
#[test]
fn the_real_system_runs_programs_by_name() {
    assert_eq!(
        System::Real.path(Program::SystemProfiler).unwrap(),
        PathBuf::from("system_profiler")
    );
    assert_eq!(System::Real.script("ioreg -n Root"), "ioreg -n Root");
    assert!(matches!(System::Real.power(), crate::awake::Power::Powerd));
}

/// An unset variable means the machine's own programs, and an empty one is
/// refused.
#[test]
fn reads_the_variable() {
    assert_eq!(System::from_setting(None).unwrap(), System::Real);
    assert_eq!(
        System::from_setting(Some(OsStr::new("/t/system"))).unwrap(),
        System::StandIn(PathBuf::from("/t/system"))
    );
    assert!(System::from_setting(Some(OsStr::new(""))).is_err());
}
