//! What `builds host setup` puts on a Linux host before builds' own setup
//! runs there, and how it puts builds there.
//!
//! A Mac host gets this Mac's own `builds` program; a Linux host cannot run
//! it, so setup sends the sources this program carries (`SOURCE`, build.rs)
//! and the host builds them, and `builds version` there names the same
//! sources as here. Before that, setup installs what the host's builds need:
//!
//! - the Debian packages builds itself needs (`PACKAGES`) and those the
//!   repos' `.build-dirs` name in `linux-package` lines, through sudo;
//! - lingering services for the user (`loginctl enable-linger`), through
//!   sudo, so systemd runs the sccache server while no one is logged in;
//! - rustup with the stable toolchain (builds installs the toolchain each
//!   repo's `rust-toolchain.toml` pins before a job needs it,
//!   rust_toolchain.rs);
//! - sccache at the version builds pins (`SCCACHE_VERSION`), from its release;
//! - a clone of each repo that has a build root on this Mac, made empty when
//!   the host has none: each job sends its commit there.

use crate::fail;
use crate::fail::Result;
use crate::host::{self, Host, Ssh, place, sh_quote};
use crate::places::Places;
use crate::remote::Transport;

/// The Debian packages builds itself needs on a Linux host: a C compiler and
/// linker for rustc and the crates' build scripts, git for the jobs'
/// commits, curl for rustup, sccache and cargo-nextest, lsof for the sccache
/// server's port, and rsync for builds' sources.
const PACKAGES: [&str; 7] = [
    "build-essential",
    "pkg-config",
    "git",
    "curl",
    "ca-certificates",
    "lsof",
    "rsync",
];

/// The sccache version a Linux host runs, from sccache's release.
pub const SCCACHE_VERSION: &str = "0.13.0";

/// Every Debian package a Linux host needs: `PACKAGES`, then those each repo
/// with a build root on this Mac names in its main checkout's `.build-dirs`,
/// once each.
pub fn packages(places: &Places) -> Result<Vec<String>> {
    let mut all: Vec<String> = PACKAGES.iter().map(|name| name.to_string()).collect();
    for repo in places.repo_folder_names()? {
        let top = places.home.join(host::clone_path(&repo));
        for name in crate::manifest::linux_packages(&top)? {
            if !all.contains(&name) {
                all.push(name);
            }
        }
    }
    Ok(all)
}

/// The repos whose clones a host needs: each with a build root on this Mac
/// and a main checkout at ~/dev/<repo>.
fn repos(places: &Places) -> Result<Vec<String>> {
    Ok(places
        .repo_folder_names()?
        .into_iter()
        .filter(|repo| {
            places
                .home
                .join(host::clone_path(repo))
                .join(".git")
                .exists()
        })
        .collect())
}

/// Installs on Linux host <host> what its builds need (the module note), and
/// returns what it did, a line each. The sudo steps run only with a person
/// at this terminal (<at_terminal>), as `host::sudo_step` says.
pub fn prepare(places: &Places, ssh: &Ssh, host: &Host, at_terminal: bool) -> Result<Vec<String>> {
    let transport = Transport { places, ssh, host };
    let name = &host.name;
    let mut said = Vec::new();
    let wanted = packages(places)?;
    let missing = missing_packages(&transport, &wanted)?;
    if missing.is_empty() {
        said.push("has the packages builds needs".to_string());
    } else {
        host::sudo_step(
            name,
            at_terminal,
            &format!("{} lacks packages: {}", place(name), missing.join(" ")),
            &[
                "sudo apt-get update".to_string(),
                format!(
                    "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
                    missing.join(" ")
                ),
            ],
            &mut |script| host::run_at_terminal(ssh, host, script),
        )?;
        said.push(format!("installed {}", missing.join(" ")));
    }
    let user = transport.capture("id -un", "asking for the user")?;
    let user = user.trim();
    let linger = transport.capture(
        &format!("loginctl show-user {} -p Linger --value", sh_quote(user)),
        "asking whether the user's services linger",
    )?;
    if linger.trim() == "yes" {
        said.push(format!("{user}'s services run while {user} is logged out"));
    } else {
        host::sudo_step(
            name,
            at_terminal,
            &format!(
                "systemd on {} stops {user}'s services, the sccache server among them, when {user} logs out",
                place(name)
            ),
            &[format!("sudo loginctl enable-linger {user}")],
            &mut |script| host::run_at_terminal(ssh, host, script),
        )?;
        said.push(format!("{user}'s services run while {user} is logged out"));
    }
    said.push(
        transport
            .capture(&tools_script(), "installing rustup and sccache")?
            .trim()
            .to_string(),
    );
    for repo in repos(places)? {
        said.push(
            transport
                .capture(
                    &format!(
                        "clone=\"$HOME\"/{clone}; if [ -e \"$clone/.git\" ]; then echo \"has a clone of {repo}\"; else git init -q \"$clone\" && echo \"made an empty clone of {repo}, which each job's commit fills\"; fi",
                        clone = sh_quote(&host::clone_path(&repo))
                    ),
                    "making the repo's clone",
                )?
                .trim()
                .to_string(),
        );
    }
    Ok(said)
}

/// Of <wanted>, the Debian packages the host lacks.
fn missing_packages(transport: &Transport, wanted: &[String]) -> Result<Vec<String>> {
    let words: Vec<String> = wanted.iter().map(|name| sh_quote(name)).collect();
    let script = format!(
        "for p in {}; do dpkg-query -W -f='${{Status}}' \"$p\" 2>/dev/null | grep -q 'ok installed' || echo \"$p\"; done",
        words.join(" ")
    );
    Ok(transport
        .capture(&script, "looking for the packages builds needs")?
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect())
}

/// Installs rustup with the stable toolchain when the host has none, and
/// sccache `SCCACHE_VERSION` into ~/.cargo/bin when the host has another
/// version or none, and says what it did.
fn tools_script() -> String {
    format!(
        r#"set -e
if [ -x "$HOME/.cargo/bin/rustup" ]; then
  echo "has rustup"
else
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable >&2
  echo "installed rustup with the stable toolchain"
fi
if [ "$("$HOME/.cargo/bin/sccache" --version 2>/dev/null)" = "sccache {SCCACHE_VERSION}" ]; then
  echo "has sccache {SCCACHE_VERSION}"
else
  part=$(mktemp -d "$HOME/.cargo/bin/.sccache.XXXXXX")
  trap 'rm -rf "$part"' EXIT
  curl -fsSL "https://github.com/mozilla/sccache/releases/download/v{SCCACHE_VERSION}/sccache-v{SCCACHE_VERSION}-$(uname -m)-unknown-linux-musl.tar.gz" | tar -xzf - -C "$part" --strip-components 1
  chmod 755 "$part/sccache"
  mv "$part/sccache" "$HOME/.cargo/bin/sccache"
  echo "installed sccache {SCCACHE_VERSION}"
fi"#
    )
}

/// Builds the sources this program carries on Linux host <host> and puts
/// the build at ~/.local/bin/builds there, in one rename, links each name of
/// `links::LINKS` to it there, each in one rename too, and creates the host's
/// builds root, which it returns. The host keeps the sources in
/// ~/.local/share/builds/source, rewriting only the files that changed, and
/// its build folder beside them, so cargo there rebuilds only what changed.
/// Fails when the build there does not name the sources this program names.
pub fn install_builds(ssh: &Ssh, host: &Host) -> Result<String> {
    let script = format!(
        r#"{root}set -e
share="$HOME/.local/share/builds"
bin="$HOME/.local/bin"
links=
new=$(mktemp -d)
trap 'rm -rf "$new" $links' EXIT
tar -xf - -C "$new"
mkdir -p "$share/source" "$bin" "$BUILDS_ROOT"
rsync -rc --delete "$new/" "$share/source/"
cargo build --release --locked --quiet --manifest-path "$share/source/builds/Cargo.toml" --target-dir "$share/target" >&2
part=$(mktemp "$bin/builds.XXXXXX")
cp "$share/target/release/builds" "$part"
chmod 755 "$part"
# set -e stops at no failure inside an && list but its last command.
mv "$part" "$bin/builds"{links} || exit 1
echo "version=$("$bin/builds" version)"
echo "root=$BUILDS_ROOT""#,
        root = crate::remote::root_script(host),
        links = crate::host_setup::link_steps()
    );
    let printed = crate::host_setup::send(ssh, host, "builds' sources", crate::SOURCE, &script)?;
    let built = host::answer_value(&printed, "version").unwrap_or("");
    if built != crate::version() {
        fail!(
            "host {} built builds from this Mac's sources, and its build says {built:?}, not {:?}",
            host.name,
            crate::version()
        );
    }
    match host::answer_value(&printed, "root") {
        Some(root) if !root.is_empty() => Ok(root.to_string()),
        _ => fail!(
            "host {} did not say where its builds root is: {printed:?}",
            host.name
        ),
    }
}
