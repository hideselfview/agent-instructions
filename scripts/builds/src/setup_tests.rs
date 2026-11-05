use super::*;
use crate::claim::write_claim;
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
    assert_eq!(std::fs::read_link(top.join("target")).unwrap(), root.join("target"));
    assert_eq!(std::fs::read_link(top.join("nested/out")).unwrap(), root.join("nested/out"));
    assert_eq!(std::fs::read_to_string(root.join(".checkout")).unwrap(), format!("{}\n", top.display()));

    fill(&root.join("target/blob"), 2048);
    fill(&root.join("nested/out/small"), 100);
    setup(&tool, &checkout).unwrap();
    assert!(root.join("target").is_dir(), "the folder over its cap was not recreated");
    assert!(!root.join("target/blob").exists(), "the folder over its cap was kept");
    assert!(root.join("nested/out/small").exists(), "a folder under its cap was deleted");
}

#[test]
fn a_claimed_worktree_keeps_its_folder_over_its_cap() {
    let scratch = Scratch::new("setup-claimed");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "target 1M\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    git(&main, &["worktree", "add", "-q", "--detach", &side.display().to_string()]);
    let checkout = Checkout::find(&side.display().to_string()).unwrap();
    write_claim(&checkout, "agent A", tool.now, None, None).unwrap();
    setup(&tool, &checkout).unwrap();
    let root = tool.places.root.join("demo/side-1");
    fill(&root.join("target/blob"), 2048);
    setup(&tool, &checkout).unwrap();
    assert!(root.join("target/blob").exists(), "a claimed worktree's folder was deleted");
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
        Err(crate::fail::Fail::Message(message)) => assert!(message.contains("which builds keeps for something else")),
        other => panic!("{other:?}"),
    }
}
