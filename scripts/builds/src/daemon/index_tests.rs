use super::*;
use crate::testing::{Scratch, fill};

/// A fresh index of the volume at <root>.
fn walked(root: &Path) -> Index {
    let meta = std::fs::symlink_metadata(root).unwrap();
    let index = Mutex::new(Index::new(root, &meta));
    walk(&index, ROOT, root, 4);
    index.into_inner().unwrap()
}

fn measured(index: &Index, path: &Path) -> Measure {
    index.measure(
        index
            .lookup(path)
            .unwrap_or_else(|| panic!("{} is not in the index", path.display())),
    )
}

/// A folder walked again on its own and grafted in leaves the index as a
/// walk of the whole volume would: new, removed and rewritten files, a clone,
/// a file linked from outside the folder, and entries whose names the volume
/// root leaves out.
#[test]
fn a_grafted_folder_equals_a_fresh_walk() {
    let scratch = Scratch::new("graft");
    let root = std::fs::canonicalize(&scratch.dir).unwrap().join("volume");
    let folder = root.join("demo/side-1");
    let deps = folder.join("target/debug/deps");
    fill(&deps.join("libone.rlib"), 1500);
    fill(&deps.join("libold.rlib"), 300);
    fill(&folder.join("target/debug/incremental/one-1/part.o"), 200);
    fill(&root.join("demo/shared/a"), 1200);
    std::fs::hard_link(root.join("demo/shared/a"), deps.join("a-link")).unwrap();
    fill(&root.join("demo/side-2/target/b"), 100);
    // The shared folder and its file were written long ago, so a rewrite
    // through the link is a later second however fast the test runs.
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for old in ["demo/shared/a", "demo/shared"] {
        std::fs::File::open(root.join(old))
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
    }
    let mut index = walked(&root);

    // What the index missed, all inside the folder or through a link into it.
    fill(&deps.join("libtwo.rlib"), 2048);
    let status = std::process::Command::new("cp")
        .arg("-c")
        .arg(deps.join("libtwo.rlib"))
        .arg(folder.join("target/debug/libtwo.rlib"))
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::remove_file(deps.join("libold.rlib")).unwrap();
    std::fs::remove_dir_all(folder.join("target/debug/incremental/one-1")).unwrap();
    fill(&folder.join("target/debug/incremental/two-1/part.o"), 100);
    fill(&deps.join("a-link"), 1600);
    fill(&folder.join(".trash/kept"), 10);
    fill(&folder.join(".DS_Store"), 4);
    assert_ne!(
        measured(&index, &folder),
        measured(&walked(&root), &folder),
        "the index already saw the changes"
    );

    let meta = std::fs::symlink_metadata(&folder).unwrap();
    let side = Mutex::new(Index::of_folder(&folder, &meta));
    walk(&side, ROOT, &folder, 4);
    let side = side.into_inner().unwrap();
    let id = index.lookup(&folder).unwrap();
    index.graft(id, &side);

    let fresh = walked(&root);
    for path in [
        folder.clone(),
        deps.clone(),
        folder.join("target/debug/incremental"),
        root.join("demo/shared"),
        root.join("demo"),
        root.clone(),
    ] {
        assert_eq!(
            measured(&index, &path),
            measured(&fresh, &path),
            "{} differs from a fresh walk",
            path.display()
        );
    }
    assert!(
        index.lookup(&folder.join(".trash/kept")).is_some(),
        "a .trash below the volume root was left out"
    );
    assert!(
        index.lookup(&folder.join(".DS_Store")).is_some(),
        "a .DS_Store below the volume root was left out"
    );
    assert_eq!(index.len(), fresh.len());
    assert_eq!(index.linked.len(), fresh.linked.len());
    assert_eq!(
        index
            .linked
            .get(&index.ino(index.lookup(&deps.join("a-link")).unwrap()))
            .map(|l| l.names),
        Some(2)
    );
}
