use super::*;
use crate::evict::Evicted;
use crate::testing::Scratch;

/// A tool and the checkout of a repo whose manifest names `target`, whose
/// build root `make` made.
fn made(scratch: &Scratch) -> (Tool, Checkout, PathBuf) {
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    crate::testing::repo(&main, "target 1G\n");
    let checkout = Checkout::find(&main.display().to_string()).unwrap();
    let root = make(&tool, &checkout).unwrap();
    (tool, checkout, root)
}

#[test]
fn a_made_root_names_its_checkout_and_is_changed() {
    let scratch = Scratch::new("roots-make");
    let (tool, checkout, root) = made(&scratch);
    assert_eq!(root, checkout.build_root(&tool.places));
    assert!(Root::is_at(&root));
    assert_eq!(Root::named_top(&root).unwrap(), Some(checkout.top.clone()));
    assert_eq!(tool.changed.roots(), vec![root]);
}

#[test]
fn a_deletion_adds_the_root_that_held_the_folder_and_none_outside_a_root() {
    let scratch = Scratch::new("roots-delete");
    let (_, _, root) = made(&scratch);
    let tool = scratch.tool(0);
    let outside = scratch.dir.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    assert_eq!(
        delete(&tool, &outside, Recreate::No, Removal::Now).unwrap(),
        Evicted::Deleted
    );
    assert_eq!(tool.changed.roots(), Vec::<PathBuf>::new());
    let target = root.join("target");
    std::fs::create_dir_all(target.join("debug")).unwrap();
    assert_eq!(
        delete(&tool, &target, Recreate::Yes, Removal::Now).unwrap(),
        Evicted::Deleted
    );
    assert!(!target.join("debug").exists());
    assert_eq!(tool.changed.roots(), vec![root]);
}

#[test]
fn the_changed_roots_are_measured_once_the_command_ends_and_a_removed_one_loses_its_record() {
    let scratch = Scratch::new("roots-record");
    let (tool, _, root) = made(&scratch);
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/file"), vec![0u8; 20_000]).unwrap();
    tool.changed.record(&tool.places);
    let sizes = crate::sizes::read(&tool.places, &root).unwrap();
    assert!(sizes["target"].kib >= 20, "{sizes:?}");
    let removing = scratch.tool(0);
    assert_eq!(
        delete(&removing, &root, Recreate::No, Removal::Now).unwrap(),
        Evicted::Deleted
    );
    assert_eq!(removing.changed.roots(), vec![root.clone()]);
    removing.changed.record(&removing.places);
    assert!(crate::sizes::read(&tool.places, &root).unwrap().is_empty());
}

/// The daemon hears once, when the command ends, of each root it changed:
/// for a removed root, after its marker went, so the daemon stops listing
/// the checkout.
#[test]
fn the_daemon_hears_once_of_a_removed_root_after_its_marker_is_gone() {
    // A socket's path must fit in SUN_LEN, which the scratch folder's does not.
    let short = crate::testing::short_dir("bt-roots");
    std::fs::create_dir_all(&short).unwrap();
    let scratch = Scratch::new("roots-tell-daemon");
    let mut tool = scratch.tool(0);
    tool.places.root = short.clone();
    let root = short.join("demo/side-1");
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join(".checkout"), "/somewhere").unwrap();
    std::fs::create_dir_all(tool.places.daemon_dir()).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(tool.places.daemon_socket()).unwrap();
    let (heard, told) = std::sync::mpsc::channel();
    let marker = root.join(".checkout");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut line = String::new();
            std::io::BufRead::read_line(
                &mut std::io::BufReader::new(stream.try_clone().unwrap()),
                &mut line,
            )
            .unwrap();
            heard.send((line, marker.exists())).unwrap();
            let done = serde_json::to_string(&crate::daemon::Reply::Done).unwrap();
            std::io::Write::write_all(&mut stream, format!("{done}\n").as_bytes()).unwrap();
        }
    });
    assert_eq!(
        delete(&tool, &root, Recreate::No, Removal::Now).unwrap(),
        Evicted::Deleted
    );
    assert_eq!(
        told.try_iter().count(),
        0,
        "the daemon heard before the command ended"
    );
    tool.changed.record(&tool.places);
    let messages: Vec<(String, bool)> = told.try_iter().collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    let (line, marker_there) = &messages[0];
    assert!(line.contains("side-1"), "{line}");
    assert!(!marker_there, "the daemon heard while the marker was there");
    let _ = std::fs::remove_dir_all(&short);
}
