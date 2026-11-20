//! Names the sources this program is built from: `SHOTS_SOURCE` is a hash of
//! every file cargo builds it from, here and in the boards library beside it
//! (version.rs). Two builds of the same sources have the same version, on any
//! host, so a run's findings name the checker that made them (`builds-shots
//! version`).

include!("version.rs");

fn main() {
    let here =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let scripts = here.parent().expect("the crate is in the scripts folder");
    println!("cargo:rustc-env=SHOTS_SOURCE={}", shots_version(scripts));
}
