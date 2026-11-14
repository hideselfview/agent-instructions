use super::*;
use crate::checkout::Checkout;
use crate::lane::Lane;
use crate::prune::prune_root;
use crate::setup::setup;
use crate::testing::{Scratch, age_all, fill, repo};

/// An Xcode tree as xcodebuild leaves it, with <kib> of products.
fn xcode_tree(path: &Path, kib: usize) {
    std::fs::create_dir_all(path.join("Logs/Build")).unwrap();
    std::fs::create_dir_all(path.join("Build/Products")).unwrap();
    std::fs::write(path.join("info.plist"), "plist").unwrap();
    fill(&path.join("Build/Products/app"), kib);
}

fn swiftpm_folder(path: &Path) {
    for dir in ["checkouts", "repositories", "artifacts"] {
        std::fs::create_dir_all(path.join(dir)).unwrap();
    }
    std::fs::write(path.join("workspace-state.json"), "{}").unwrap();
    for config in ["debug", "release"] {
        fill(
            &path
                .join("arm64-apple-macosx")
                .join(config)
                .join("Lib.build/out"),
            8,
        );
    }
    std::os::unix::fs::symlink("arm64-apple-macosx/debug", path.join("debug")).unwrap();
    fill(&path.join("checkouts/dep/debug/file"), 1);
}

#[test]
fn finds_xcode_trees_and_swiftpm_configurations_but_not_what_they_share() {
    let scratch = Scratch::new("trees-find");
    let app = scratch.dir.join("app/.build");
    xcode_tree(&app.join("runApp"), 8);
    xcode_tree(&app.join("nested/unsigned"), 8);
    fill(&app.join("SourcePackages/checkouts/Sparkle/file"), 8);
    std::fs::write(app.join("SourcePackages/workspace-state.json"), "{}").unwrap();
    let found = trees_in(&app).unwrap();
    assert_eq!(
        found,
        vec![
            Tree {
                path: app.join("nested/unsigned"),
                kind: Kind::Xcode
            },
            Tree {
                path: app.join("runApp"),
                kind: Kind::Xcode
            },
        ]
    );

    let kit = scratch.dir.join("kit/.build");
    swiftpm_folder(&kit);
    assert_eq!(
        trees_in(&kit).unwrap(),
        vec![
            Tree {
                path: kit.join("arm64-apple-macosx/debug"),
                kind: Kind::SwiftPm
            },
            Tree {
                path: kit.join("arm64-apple-macosx/release"),
                kind: Kind::SwiftPm
            },
        ]
    );

    let target = scratch.dir.join("target");
    std::fs::create_dir_all(target.join("debug/deps")).unwrap();
    std::fs::write(target.join(".rustc_info.json"), "{}").unwrap();
    xcode_tree(&target.join("debug/stray"), 1);
    assert_eq!(
        trees_in(&target).unwrap(),
        vec![],
        "cargo's folder is aged by its own pass"
    );
}

#[test]
fn setup_deletes_an_unused_tree_beside_a_live_one() {
    let scratch = Scratch::new("trees-setup");
    let tool = scratch.tool(0);
    let top = scratch.dir.join("demo");
    repo(&top, "app/.build 1G\n");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    setup(&tool, &checkout).unwrap();
    let build = tool.places.root.join("demo/main/app/.build");
    xcode_tree(&build.join("old"), 8);
    xcode_tree(&build.join("live"), 8);
    fill(&build.join("SourcePackages/checkouts/Sparkle/file"), 8);
    age_all(&build.join("old"), 24 * 3 + 1);
    age_all(&build.join("SourcePackages"), 24 * 10);

    setup(&tool, &checkout).unwrap();
    assert!(
        !build.join("old").exists(),
        "a tree no build used for three days was kept"
    );
    assert!(
        build.join("live/Build/Products/app").exists(),
        "a tree built into today was deleted"
    );
    assert!(
        build.join("SourcePackages/checkouts/Sparkle/file").exists(),
        "the shared packages were deleted"
    );
}

#[test]
fn a_tree_used_within_three_days_stays_and_a_dry_run_deletes_nothing() {
    let scratch = Scratch::new("trees-dry");
    let tool = scratch.tool(0);
    let top = scratch.dir.join("demo");
    repo(&top, "app/.build 1G\n");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    setup(&tool, &checkout).unwrap();
    let root = tool.places.root.join("demo/main");
    let build = root.join("app/.build");
    xcode_tree(&build.join("recent"), 8);
    xcode_tree(&build.join("old"), 8);
    age_all(&build.join("recent"), 24 * 3 - 1);
    age_all(&build.join("old"), 24 * 4);

    prune_root(&tool, &root, &checkout, Run::Dry, 0, 0).unwrap();
    assert!(build.join("old").exists(), "a dry run deleted a tree");
    prune_root(&tool, &root, &checkout, Run::Real, 0, 0).unwrap();
    assert!(build.join("recent").exists());
    assert!(!build.join("old").exists());
}

#[test]
fn an_old_swiftpm_configuration_goes_and_its_link_target_alone() {
    let scratch = Scratch::new("trees-swiftpm");
    let tool = scratch.tool(0);
    let top = scratch.dir.join("demo");
    repo(&top, "kit/.build 1G\n");
    let checkout = Checkout::find(&top.display().to_string()).unwrap();
    setup(&tool, &checkout).unwrap();
    let build = tool.places.root.join("demo/main/kit/.build");
    swiftpm_folder(&build);
    age_all(&build.join("arm64-apple-macosx/release"), 24 * 5);
    setup(&tool, &checkout).unwrap();
    assert!(!build.join("arm64-apple-macosx/release").exists());
    assert!(
        build
            .join("arm64-apple-macosx/debug/Lib.build/out")
            .exists()
    );
    assert!(
        build.join("checkouts/dep/debug/file").exists(),
        "a dependency's folder was taken for a build"
    );
}

#[test]
fn a_kept_tree_stays_in_main_and_the_app_checkout_but_not_in_another_worktree() {
    let scratch = Scratch::new("trees-kept");
    let tool = scratch.tool(0);
    let main = scratch.dir.join("demo");
    repo(&main, "app/.build 1G\nkeep app/.build/run\n");
    let side = scratch.dir.join("demo-worktrees/side-1");
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &side.display().to_string(),
        ],
    );
    let other = scratch.dir.join("demo-worktrees/side-2");
    crate::testing::git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &other.display().to_string(),
        ],
    );
    std::fs::create_dir_all(tool.places.root.join("demo")).unwrap();
    std::fs::write(
        tool.places.app_checkouts_file("demo", Lane::Mac),
        "side-1\n",
    )
    .unwrap();

    for (top, name, kept) in [
        (&main, "main", true),
        (&side, "side-1", true),
        (&other, "side-2", false),
    ] {
        let checkout = Checkout::find(&top.display().to_string()).unwrap();
        setup(&tool, &checkout).unwrap();
        let build = tool.places.root.join("demo").join(name).join("app/.build");
        xcode_tree(&build.join("run"), 8);
        xcode_tree(&build.join("unsigned"), 8);
        age_all(&build.join("run"), 24 * 10);
        age_all(&build.join("unsigned"), 24 * 10);
        setup(&tool, &checkout).unwrap();
        assert_eq!(build.join("run").exists(), kept, "{name}: the kept tree");
        assert!(
            !build.join("unsigned").exists(),
            "{name}: an unkept unused tree was kept"
        );
    }
}
