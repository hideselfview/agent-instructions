//! A stand-in for launchctl, for the integration tests whose builds commands
//! read or set up a host's services. A test's temporary HOME gives launchd no
//! domain of its own, so the real launchctl would act on the jobs of the
//! person who runs the tests.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// launchctl, as a program of a test's stand-in system (`common::system`).
pub struct FakeLaunchctl {
    record: PathBuf,
}

impl FakeLaunchctl {
    /// Writes the program into the stand-in system <system>. It records each
    /// command it gets. It answers `print` with text that builds cannot read
    /// as a job, so a host setup fails at its first read of the job, before it
    /// stops the server on sccache's port, which is the person's. It fails
    /// every other command.
    pub fn new(system: &Path) -> FakeLaunchctl {
        let record = system.join("launchctl.ran");
        let program = system.join("launchctl");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\necho \"a test's stand-in for launchctl\"\n[ \"$1\" = print ]\n",
                record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        FakeLaunchctl { record }
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
