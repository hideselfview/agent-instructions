use super::*;
use crate::claim::write_claim;
use crate::lane::Lane;
use crate::testing::{Scratch, fill, git, repo};

#[test]
fn links_each_path_and_deletes_a_folder_over_its_cap() {
    let scratch = Scratch::new("setup-links");
    let tool = scratch.tool(0);
    let top = scratch.dir.join("demo");
    repo(&top, "target 1M\nnested/out 1M\n");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    setup(&tool, &checkout).unwrap();
    let root = tool.places.root.join("demo/main");
    assert_eq!(
        std::fs::read_link(top.join("target")).unwrap(),
        root.join("target")
    );
    assert_eq!(
        std::fs::read_link(top.join("nested/out")).unwrap(),
        root.join("nested/out")
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".checkout")).unwrap(),
        format!("{}\n", top.display())
    );

    fill(&root.join("target/blob"), 2048);
    fill(&root.join("nested/out/small"), 100);
    setup(&tool, &checkout).unwrap();
    assert!(
        root.join("target").is_dir(),
        "the folder over its cap was not recreated"
    );
    assert!(
        !root.join("target/blob").exists(),
        "the folder over its cap was kept"
    );
    assert!(
        root.join("nested/out/small").exists(),
        "a folder under its cap was deleted"
    );
}

#[test]
fn a_claimed_worktree_keeps_its_folder_over_its_cap() {
    let scratch = Scratch::new("setup-claimed");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1M\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &side.display().to_string(),
        ],
    );
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    write_claim(&checkout, "agent A", tool.now, None, None).unwrap();
    setup(&tool, &checkout).unwrap();
    let root = tool.places.root.join("demo/side-1");
    fill(&root.join("target/blob"), 2048);
    setup(&tool, &checkout).unwrap();
    assert!(
        root.join("target/blob").exists(),
        "a claimed worktree's folder was deleted"
    );
}

#[test]
fn the_app_checkout_takes_its_own_cap() {
    let scratch = Scratch::new("setup-app-cap");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 4M app 1M\n");
    let mut roots = Vec::new();
    for name in ["side-1", "side-2"] {
        let side = scratch.dir.join("demo-worktrees").join(name);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &side.display().to_string(),
            ],
        );
        let checkout = Checkout::find(&side.display().to_string()).unwrap();
        setup(&tool, &checkout).unwrap();
        roots.push((checkout, tool.places.root.join("demo").join(name)));
    }
    std::fs::write(
        tool.places.app_checkouts_file("demo", Lane::Mac),
        "side-1\n",
    )
    .unwrap();
    for (checkout, root) in &roots {
        fill(&root.join("target/blob"), 2048);
        setup(&tool, checkout).unwrap();
    }
    assert!(
        !roots[0].1.join("target/blob").exists(),
        "the app checkout's folder over its app cap was kept"
    );
    assert!(
        roots[1].1.join("target/blob").exists(),
        "another worktree's folder under its cap was deleted"
    );
}

#[test]
fn a_real_directory_becomes_a_link() {
    let scratch = Scratch::new("setup-real-dir");
    let tool = scratch.tool(0);
    let top = scratch.dir.join("demo");
    repo(&top, "target 1G\n");
    fill(&top.join("target/old"), 10);
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    setup(&tool, &checkout).unwrap();
    assert!(top.join("target").is_symlink());
    assert!(!top.join("target/old").exists());
}

#[test]
fn a_repo_cannot_take_a_volume_folder_name() {
    let scratch = Scratch::new("setup-reserved");
    let tool = scratch.tool(0);
    let top = scratch.dir.join("caches");
    repo(&top, "target 1G\n");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    match setup(&tool, &checkout) {
        Err(crate::fail::Fail::Message(message)) => {
            assert!(message.contains("which builds keeps for something else"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_lane_checkout_takes_the_app_cap_only_on_the_folders_its_lane_builds_into() {
    let scratch = Scratch::new("setup-lane-cap");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(
        &main,
        "xcode 4M app 1M lane mac\ngradle 4M app 1M lane android\n",
    );
    let mut roots = Vec::new();
    for (name, lane) in [("side-1", Lane::Mac), ("side-2", Lane::Android)] {
        let side = scratch.dir.join("demo-worktrees").join(name);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &side.display().to_string(),
            ],
        );
        let checkout = Checkout::find(&side.display().to_string()).unwrap();
        setup(&tool, &checkout).unwrap();
        std::fs::write(
            tool.places.app_checkouts_file("demo", lane),
            format!("{name}\n"),
        )
        .unwrap();
        roots.push((checkout, tool.places.root.join("demo").join(name)));
    }
    for (checkout, root) in &roots {
        fill(&root.join("xcode/blob"), 2048);
        fill(&root.join("gradle/blob"), 2048);
        setup(&tool, checkout).unwrap();
    }
    assert!(
        !roots[0].1.join("xcode/blob").exists(),
        "the mac lane's folder over its app cap was kept"
    );
    assert!(
        roots[0].1.join("gradle/blob").exists(),
        "the other lane's folder, under its ordinary cap, was deleted"
    );
    assert!(
        !roots[1].1.join("gradle/blob").exists(),
        "the android lane's folder over its app cap was kept"
    );
    assert!(roots[1].1.join("xcode/blob").exists());
}
