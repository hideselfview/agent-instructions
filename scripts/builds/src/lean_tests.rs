use super::*;
use crate::testing::{Scratch, repo};
use std::os::unix::fs::PermissionsExt;

#[test]
fn each_listed_lake_folder_beside_a_lean_toolchain_file_pins_its_toolchain_once() {
    let scratch = Scratch::new("lean-packages");
    let top = scratch.dir.join("demo");
    repo(
        &top,
        "target 1G\nproofs/a/.lake 1G\nproofs/b/.lake 1G\n.lake 1G\nproofs/d/.lake 1G\n",
    );
    std::fs::create_dir_all(top.join("proofs/a")).unwrap();
    std::fs::write(
        top.join("proofs/a/lean-toolchain"),
        "leanprover/lean4:v4.34.1\n",
    )
    .unwrap();
    std::fs::write(top.join("lean-toolchain"), "leanprover/lean4:v4.36.0").unwrap();
    std::fs::create_dir_all(top.join("proofs/d")).unwrap();
    std::fs::write(
        top.join("proofs/d/lean-toolchain"),
        "leanprover/lean4:v4.34.1",
    )
    .unwrap();
    assert_eq!(
        toolchains(&top).unwrap(),
        vec![
            "leanprover/lean4:v4.34.1".to_string(),
            "leanprover/lean4:v4.36.0".to_string()
        ],
        "proofs/d pins proofs/a's toolchain, proofs/b has no lean-toolchain, and target is no .lake folder"
    );
    std::fs::write(top.join("lean-toolchain"), "\n").unwrap();
    assert!(
        toolchains(&top).is_err(),
        "an empty lean-toolchain names none"
    );
}

/// Writes into <dir> an `elan-init` that installs, under `$ELAN_HOME`, an
/// `elan` that lists the toolchains in `$ELAN_HOME/list` and installs one by
/// adding it there, unless it is in `$ELAN_HOME/refused`.
fn fake_installer(dir: &Path) -> Result<()> {
    let installer = dir.join("elan-init");
    std::fs::write(
        &installer,
        r#"#!/bin/sh
[ "$*" = "-y --no-modify-path --default-toolchain none" ] || exit 3
mkdir -p "$ELAN_HOME/bin"
cat > "$ELAN_HOME/bin/elan" <<'ELAN'
#!/bin/sh
case "$1 $2" in
  "toolchain list") if [ -s "$ELAN_HOME/list" ]; then sed '1s/$/ (default)/' "$ELAN_HOME/list"; else echo "no installed toolchains"; fi ;;
  "toolchain install") grep -qx "$3" "$ELAN_HOME/refused" 2>/dev/null && exit 1; echo "$3" >> "$ELAN_HOME/list" ;;
  *) exit 2 ;;
esac
ELAN
chmod 755 "$ELAN_HOME/bin/elan"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&installer, std::fs::Permissions::from_mode(0o755)).unwrap();
    Ok(())
}

#[test]
fn setup_installs_elan_and_each_missing_toolchain_once() {
    let scratch = Scratch::new("lean-ensure");
    let home = scratch.dir.join("home");
    let locks = scratch.dir.join("caches");
    let wanted = vec![
        "leanprover/lean4:v4.34.1".to_string(),
        "leanprover/lean4:v4.36.0".to_string(),
    ];
    ensure_toolchains(&home, &locks, &wanted, fake_installer).unwrap();
    let list = home.join(".elan/list");
    assert_eq!(
        std::fs::read_to_string(&list).unwrap(),
        "leanprover/lean4:v4.34.1\nleanprover/lean4:v4.36.0\n"
    );
    let left: Vec<_> = std::fs::read_dir(&home)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left, [".elan"], "the installer's folder is gone");

    // Everything is in place: nothing downloads or installs.
    ensure_toolchains(&home, &locks, &wanted, |_| panic!("downloaded again")).unwrap();
    assert_eq!(std::fs::read_to_string(&list).unwrap().lines().count(), 2);

    // A new pin installs only that toolchain, with the elan already there.
    let more = vec![wanted[0].clone(), "leanprover/lean4:v4.37.0".to_string()];
    ensure_toolchains(&home, &locks, &more, |_| panic!("downloaded again")).unwrap();
    assert_eq!(std::fs::read_to_string(&list).unwrap().lines().count(), 3);

    // A toolchain elan cannot install fails, naming it.
    std::fs::write(home.join(".elan/refused"), "leanprover/lean4:v9\n").unwrap();
    let message = ensure_toolchains(
        &home,
        &locks,
        &["leanprover/lean4:v9".to_string()],
        fake_installer,
    )
    .unwrap_err()
    .text();
    assert!(message.contains("leanprover/lean4:v9"), "{message}");
}

#[test]
fn an_installer_that_installs_no_elan_fails_and_leaves_no_folder() {
    let scratch = Scratch::new("lean-no-elan");
    let home = scratch.dir.join("home");
    let message = ensure_toolchains(
        &home,
        &scratch.dir.join("caches"),
        &["leanprover/lean4:v4.34.1".to_string()],
        |dir| {
            let installer = dir.join("elan-init");
            std::fs::write(&installer, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&installer, std::fs::Permissions::from_mode(0o755)).unwrap();
            Ok(())
        },
    )
    .unwrap_err()
    .text();
    assert!(message.contains("is missing"), "{message}");
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
}
