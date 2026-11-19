//! The programs builds installs on every host that takes cargo work, one
//! pinned version each, because a `builds cargo` run of the subcommand that
//! needs one goes only to a host that has it (`host::takes_run`): cargo-nextest
//! for `builds cargo nextest` and cargo-mutants for `builds cargo mutants`.
//! `install` is run by `builds install` here and by `builds host setup-here`
//! on each host, and `Program::probe_lines` and `Program::parse_has` are how a
//! probe reads which version a host has.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::fail;
use crate::fail::{Context, Result};
use crate::text::note;

/// A cargo subcommand's program that builds pins to one version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Program {
    /// The program's file name in `~/.cargo/bin`: `cargo-nextest`.
    pub name: &'static str,
    /// The word a probe's answer names it by: `nextest=<version>`.
    pub key: &'static str,
    /// What the program takes to print `<name> <version>` on its first line.
    pub version_args: &'static [&'static str],
    /// The version on every host and on this Mac.
    pub version: &'static str,
}

/// cargo-nextest, which `builds cargo nextest` runs.
pub const NEXTEST: Program = Program {
    name: "cargo-nextest",
    key: "nextest",
    version_args: &["nextest", "--version"],
    version: "0.9.143",
};

/// cargo-mutants, which `builds cargo mutants` runs.
pub const MUTANTS: Program = Program {
    name: "cargo-mutants",
    key: "mutants",
    version_args: &["mutants", "--version"],
    version: "27.1.0",
};

/// Where cargo finds its subcommand programs under <home>.
fn bin_dir(home: &Path) -> PathBuf {
    home.join(".cargo/bin")
}

/// What a host has of a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Has {
    /// No such program.
    Nothing,
    /// A program that reports this version.
    Version(String),
    /// A program that does not start, and what it said.
    Broken(String),
}

impl Program {
    /// The lines of a probe's script that print `<key>=<version>` from the
    /// first line of `<name> <version args>` (`cargo-nextest 0.9.143`), or
    /// `<key>_fails=<what it said>` when the program does not start. A host
    /// without the program prints nothing.
    pub fn probe_lines(&self) -> String {
        let Program {
            name,
            key,
            version_args,
            ..
        } = self;
        let args = version_args.join(" ");
        format!(
            r#"if [ -e "$HOME/.cargo/bin/{name}" ]; then if v=$("$HOME/.cargo/bin/{name}" {args} 2>&1); then echo "{key}=$(printf '%s\n' "$v" | head -n 1 | awk '{{print $2}}')"; else echo "{key}_fails=$(printf '%s' "$v" | tr '\n' ' ')"; fi; fi"#
        )
    }

    /// What a probe's answer says the host has.
    pub fn parse_has(&self, text: &str) -> Has {
        let fails = format!("{}_fails=", self.key);
        let version = format!("{}=", self.key);
        if let Some(said) = text.lines().find_map(|line| line.strip_prefix(&fails)) {
            return Has::Broken(said.trim().to_string());
        }
        match text
            .lines()
            .find_map(|line| line.strip_prefix(&version))
            .map(str::trim)
        {
            Some("") => Has::Broken(format!(
                "`{} {}` printed no version",
                self.name,
                self.version_args.join(" ")
            )),
            Some(version) => Has::Version(version.to_string()),
            None => Has::Nothing,
        }
    }

    /// What the program under <home>'s `.cargo/bin` is.
    pub fn has(&self, home: &Path) -> Has {
        let program = bin_dir(home).join(self.name);
        if !program.exists() {
            return Has::Nothing;
        }
        let output = match Command::new(&program)
            .args(self.version_args)
            .stdin(Stdio::null())
            .output()
        {
            Ok(output) => output,
            Err(error) => return Has::Broken(format!("running {}: {error}", program.display())),
        };
        if !output.status.success() {
            return Has::Broken(format!(
                "{} failed ({}): {}",
                program.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        match String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
        {
            Some(version) => Has::Version(version.to_string()),
            None => Has::Broken(format!("{} printed no version", program.display())),
        }
    }

    /// Installs the program at `version` under <home>'s `.cargo/bin` when it
    /// has another version or none, and returns whether it did. <fetch> puts
    /// the program in the folder it is given, a new one beside the program
    /// (`evict::in_temp_dir`), so that the rename stays on one volume. Fails
    /// when the program there does not start, and when the new folder cannot
    /// be created or removed.
    pub fn install_with(
        &self,
        home: &Path,
        fetch: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<bool> {
        let Program { name, version, .. } = *self;
        match self.has(home) {
            Has::Version(has) if has == version => return Ok(false),
            Has::Broken(said) => {
                fail!("the {name} under {} does not start: {said}", home.display())
            }
            Has::Version(_) | Has::Nothing => {}
        }
        let bin = bin_dir(home);
        std::fs::create_dir_all(&bin).context(|| format!("creating {}", bin.display()))?;
        crate::evict::in_temp_dir(&bin, &format!(".{name}."), |part| {
            fetch(part)?;
            let program = part.join(name);
            if !program.exists() {
                fail!("the {name} {version} download held no {name} program");
            }
            let target = bin.join(name);
            std::fs::rename(&program, &target)
                .context(|| format!("replacing {}", target.display()))?;
            match self.has(home) {
                Has::Version(has) if has == version => Ok(true),
                other => fail!(
                    "{} is {other:?} after installing {version}",
                    target.display()
                ),
            }
        })
    }

    /// Puts the program's `version` in <dir>: cargo-nextest's release build
    /// for this machine, and cargo-mutants compiled from its crate.
    fn fetch(&self, home: &Path, dir: &Path) -> Result<()> {
        if *self == NEXTEST {
            return download_nextest(dir);
        }
        let version = self.version;
        // A host's ssh shell may have no PATH to cargo.
        let cargo = bin_dir(home).join("cargo");
        let output = Command::new(if cargo.exists() {
            cargo
        } else {
            PathBuf::from("cargo")
        })
        .args(["install", "--locked", "--version", version])
        .arg("--root")
        .arg(dir)
        .arg(self.name)
        .stdin(Stdio::null())
        .output()
        .context(|| "running cargo install".to_string())?;
        if !output.status.success() {
            fail!(
                "cargo install {} {version} failed ({}): {}",
                self.name,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        // cargo install --root <dir> puts programs in <dir>/bin.
        let built = dir.join("bin").join(self.name);
        std::fs::rename(&built, dir.join(self.name))
            .context(|| format!("moving {} out of cargo's bin folder", built.display()))
    }

    /// Installs the program at `version` under <home> (`install_with`),
    /// saying so on stderr when it changed anything.
    pub fn install(&self, home: &Path) -> Result<()> {
        if self.install_with(home, |dir| self.fetch(home, dir))? {
            note(format!("installed {} {}", self.name, self.version));
        }
        Ok(())
    }
}

/// The name get.nexte.st gives the release's build for this machine: the
/// universal macOS build, or the Linux build for its processor.
fn release_platform() -> &'static str {
    match (crate::host_platform::Os::here(), std::env::consts::ARCH) {
        (crate::host_platform::Os::Mac, _) => "mac",
        (crate::host_platform::Os::Linux, "aarch64") => "linux-arm",
        (crate::host_platform::Os::Linux, _) => "linux",
    }
}

/// Downloads the release's build of cargo-nextest for this machine into
/// <dir>.
fn download_nextest(dir: &Path) -> Result<()> {
    crate::download::unpack_tarball(
        &format!(
            "https://get.nexte.st/{}/{}",
            NEXTEST.version,
            release_platform()
        ),
        dir,
        &format!("cargo-nextest {}", NEXTEST.version),
    )
}

/// Installs every program that builds pins, on this Mac or a host.
pub fn install_all(home: &Path) -> Result<()> {
    NEXTEST.install(home)?;
    MUTANTS.install(home)
}

#[cfg(test)]
#[path = "cargo_program_tests.rs"]
pub mod tests;
