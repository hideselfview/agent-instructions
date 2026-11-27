//! A stand-in for launchctl, for the integration tests whose builds commands
//! read or set up a host's services. A test's temporary HOME gives launchd no
//! domain of its own, so the real launchctl would act on the jobs of the
//! person who runs the tests.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// launchctl, as a program in a folder that a test puts first on the PATH of
/// its builds commands.
pub struct FakeLaunchctl {
    bin: PathBuf,
    record: PathBuf,
}

impl FakeLaunchctl {
    /// Writes the program into <bin>. It records each command it gets. It
    /// answers `print` with text that builds cannot read as a job, so a host
    /// setup fails at its first read of the job, before it stops the server on
    /// sccache's port, which is the person's. It fails every other command.
    pub fn new(bin: &Path) -> FakeLaunchctl {
        std::fs::create_dir_all(bin).unwrap();
        let record = bin.join("launchctl.ran");
        let program = bin.join("launchctl");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\necho \"a test's stand-in for launchctl\"\n[ \"$1\" = print ]\n",
                record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        FakeLaunchctl {
            bin: bin.to_path_buf(),
            record,
        }
    }

    /// This process's PATH with the program's folder first.
    pub fn path(&self) -> std::ffi::OsString {
        let inherited = std::env::var_os("PATH").expect("the tests have a PATH");
        std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(&inherited)),
        )
        .unwrap()
    }

    /// The arguments of each command it got, oldest first.
    pub fn ran(&self) -> Vec<String> {
        match std::fs::read_to_string(&self.record) {
            Ok(text) => text.lines().map(str::to_string).collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("reading {}: {error}", self.record.display()),
        }
    }
}
