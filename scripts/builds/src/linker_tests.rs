use super::*;
use crate::testing::Scratch;

fn words(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// A link of rustc's shape: a dylib (or not) whose exported symbols, listed
/// in <list>, are <symbols>.
fn link(scratch: &Scratch, symbols: &str, extra: &[&str]) -> Vec<OsString> {
    let list = scratch.dir.join("list");
    std::fs::write(&list, symbols).unwrap();
    let mut args = words(&["-Wl,-exported_symbols_list"]);
    args.push(OsString::from(format!("-Wl,{}", list.display())));
    args.extend(words(extra));
    args
}

#[test]
fn a_proc_macro_dylib_gets_an_install_name_of_its_file_name() {
    let scratch = Scratch::new("linker-proc-macro");
    let symbols = "___rustc_proc_macro_decls_76a49fa24ff40fe4__\n_rust_metadata_demo_76a4\n";
    let args = link(
        &scratch,
        symbols,
        &[
            "-o",
            "/a/target/debug/deps/libdemo-0123.dylib",
            "-dynamiclib",
        ],
    );
    assert_eq!(
        added_args(&args).unwrap(),
        words(&["-Wl,-install_name,@rpath/libdemo-0123.dylib"])
    );
}

#[test]
fn any_other_link_is_left_as_rustc_made_it() {
    let scratch = Scratch::new("linker-other");
    let proc_macro = "___rustc_proc_macro_decls_76a49fa24ff40fe4__\n";
    // A cdylib exports its own symbols, not rustc's proc-macro symbol.
    let cdylib = link(
        &scratch,
        "_uniffi_bae_bridge_fn_open\n",
        &["-o", "/a/libbae_bridge.dylib", "-dynamiclib"],
    );
    assert_eq!(added_args(&cdylib).unwrap(), Vec::<OsString>::new());
    let named = link(
        &scratch,
        proc_macro,
        &[
            "-o",
            "/a/libdemo.dylib",
            "-dynamiclib",
            "-Wl,-install_name,@rpath/other.dylib",
        ],
    );
    assert_eq!(added_args(&named).unwrap(), Vec::<OsString>::new());
    let executable = link(&scratch, proc_macro, &["-o", "/a/demo"]);
    assert_eq!(added_args(&executable).unwrap(), Vec::<OsString>::new());
    assert_eq!(
        added_args(&words(&["-o", "/a/libdemo.dylib", "-dynamiclib"])).unwrap(),
        Vec::<OsString>::new()
    );
}

/// The same proc-macro, compiled by rustc from the same sources into two
/// checkouts' folders, is the same bytes once linked with the added install
/// name, and differs without it.
#[test]
fn one_proc_macro_built_in_two_folders_is_the_same_bytes() {
    let scratch = Scratch::new("linker-bytes");
    let source = scratch.dir.join("registry/demo/src/lib.rs");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(
        &source,
        "extern crate proc_macro;\n#[proc_macro]\npub fn same(input: proc_macro::TokenStream) -> proc_macro::TokenStream { input }\n",
    )
    .unwrap();
    // Saves the arguments and the exported symbols rustc hands the linker,
    // then fails the link: the test links each folder itself below.
    let saver = scratch.dir.join("save-link.sh");
    std::fs::write(
        &saver,
        r#"#!/bin/sh
out="$(dirname "$0")/saved"
mkdir -p "$out"
: > "$out/args"
prev=
for a in "$@"; do
  case "$prev" in -Wl,-exported_symbols_list) cp "${a#-Wl,}" "$out/list"; a="-Wl,$out/list";; esac
  printf '%s\n' "$a" >> "$out/args"
  prev=$a
done
for a in "$@"; do case "$a" in *.o) cp "$a" "$out/";; esac; done
exit 1
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&saver, std::fs::Permissions::from_mode(0o755)).unwrap();
    let built = |folder: &str, install_name: bool| -> Vec<u8> {
        let out = scratch.dir.join(folder).join("target/debug/deps");
        std::fs::create_dir_all(&out).unwrap();
        let _ = std::fs::remove_dir_all(scratch.dir.join("saved"));
        let status = std::process::Command::new("rustc")
            .current_dir(source.parent().unwrap().parent().unwrap())
            .args(["--edition", "2021", "--crate-name", "demo"])
            .args(["--crate-type", "proc-macro", "-C", "prefer-dynamic"])
            .args(["-C", "embed-bitcode=no", "-C", "metadata=0123abcd"])
            .args(["-C", "extra-filename=-0123abcd"])
            .arg(format!("-Clinker={}", saver.display()))
            .arg("--out-dir")
            .arg(&out)
            .arg("src/lib.rs")
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "the saving linker fails the link");
        let saved = scratch.dir.join("saved");
        let args: Vec<OsString> = std::fs::read_to_string(saved.join("args"))
            .unwrap()
            .lines()
            .map(|line| {
                // The objects rustc deletes after the link are the saved
                // copies now.
                let path = Path::new(line);
                if line.ends_with(".o") {
                    saved.join(path.file_name().unwrap()).into_os_string()
                } else {
                    OsString::from(line)
                }
            })
            .collect();
        let mut added = added_args(&args).unwrap();
        if !install_name {
            added.clear();
        } else {
            assert_eq!(added.len(), 1, "a proc-macro gets its install name");
        }
        let status = std::process::Command::new("cc")
            .args(&args)
            .args(&added)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::read(out.join("libdemo-0123abcd.dylib")).unwrap()
    };
    assert_ne!(built("side-1", false), built("side-2", false));
    assert_eq!(built("side-1", true), built("side-2", true));
}
