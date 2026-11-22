use super::*;
use crate::lists::Resolution;
use crate::lists::fixtures::{db, design_drawn, design_place, design_tipped, read};
use crate::testing::{Scratch, git, repo};
use std::collections::BTreeSet;
use std::sync::mpsc;
use std::time::Duration;

const BOARD: &str = "design/mocks/Alb.dc.html";

#[test]
fn a_worktree_is_read_once_per_tip_and_again_when_its_commits_main_or_store_move() {
    let scratch = Scratch::new("design-reads-per-tip");
    let top = scratch.dir.join("side-2");
    repo(&top, "");
    std::fs::create_dir_all(top.join("design/mocks")).unwrap();
    std::fs::write(top.join(BOARD), "x").unwrap();
    git(&top, &["add", BOARD]);
    git(&top, &["commit", "-q", "-m", "Draw"]);
    let sha = boards::git_text(boards::git(&top).args(["rev-parse", "HEAD"]), "read HEAD")
        .unwrap()
        .trim()
        .to_string();
    let db = db();
    let id = design_drawn(&db, &top.display().to_string(), &[], None);
    design_tipped(&db, id, &sha);
    design_place(&db, id, BOARD, Resolution::Matches("checked".into()));
    let unrelated = design_drawn(&db, &top.display().to_string(), &[], None);
    design_tipped(&db, unrelated, &"4".repeat(40));
    design_place(
        &db,
        unrelated,
        "design/mocks/Other.dc.html",
        Resolution::Matches("checked".into()),
    );
    let mut lists = read(&db);
    lists
        .design_places
        .insert("bae".into(), vec!["design/mocks/*.dc.html".into()]);
    let git = SideRead {
        head: None,
        ahead: vec![crate::side_git::Commit {
            sha: sha.chars().take(7).collect(),
            full: sha,
            time: 1,
            authored: 1,
            subject: "Draw".into(),
        }],
        main: "1".repeat(40),
        uncommitted: 0,
        last_write: None,
    };
    let reads = DesignReads::default();
    let first = reads.of(&lists, "bae", &top, &git).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].covered_by, Some(id));
    // With its git folder gone, git can read nothing in the worktree, so
    // only the commits kept from the first read answer.
    std::fs::rename(top.join(".git"), scratch.dir.join("dot-git")).unwrap();
    let again = reads.of(&lists, "bae", &top, &git).unwrap();
    assert!(Arc::ptr_eq(&first, &again), "the same tip is read once");
    let mut written = lists.clone();
    written.version += 1;
    let mut other_commits = git.clone();
    other_commits.ahead[0].full = "2".repeat(40);
    let mut moved_main = git.clone();
    moved_main.main = "3".repeat(40);
    let tips = [
        ("other commits", &lists, &other_commits),
        ("a moved origin/main", &lists, &moved_main),
        ("a write to the store", &written, &git),
    ];
    for (what, lists, git) in tips {
        let read = reads.of(lists, "bae", &top, git);
        assert!(read.is_err(), "{what} is read again, from git: {read:?}");
    }
}

#[test]
fn a_slow_drawing_does_not_block_another_worktree_read() {
    struct BlockedRender {
        started: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl Renders for BlockedRender {
        fn alike(
            &self,
            _: &Path,
            _: &str,
            _: &str,
            places: &BTreeSet<String>,
        ) -> Result<BTreeSet<String>> {
            self.started.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            Ok(places.clone())
        }
    }

    let scratch = Scratch::new("design-reads-independent");
    let top = scratch.dir.join("side-2");
    repo(&top, "");
    std::fs::create_dir_all(top.join("design/mocks")).unwrap();
    std::fs::write(
        top.join("design/mocks/canvas.json"),
        r#"{"designSystems": [{"namespace": "demo"}], "boards": {}}"#,
    )
    .unwrap();
    std::fs::write(top.join(BOARD), "before").unwrap();
    git(&top, &["add", "design/mocks"]);
    git(&top, &["commit", "-q", "-m", "Draw"]);
    let before = crate::lists::design_paths::full_sha(&top, "HEAD").unwrap();
    std::fs::write(top.join(BOARD), "after").unwrap();
    git(&top, &["commit", "-qam", "Redraw"]);
    let after = crate::lists::design_paths::full_sha(&top, "HEAD").unwrap();
    let db = db();
    let id = design_drawn(&db, &top.display().to_string(), &[], None);
    design_tipped(&db, id, &before);
    design_place(&db, id, BOARD, Resolution::Matches("checked".into()));
    let mut lists = read(&db);
    lists
        .design_places
        .insert("bae".into(), vec!["design/mocks/*.dc.html".into()]);
    let git = SideRead {
        head: None,
        ahead: vec![crate::side_git::Commit {
            sha: after.chars().take(7).collect(),
            full: after,
            time: 1,
            authored: 1,
            subject: "Redraw".into(),
        }],
        main: before,
        uncommitted: 0,
        last_write: None,
    };
    let other = scratch.dir.join("side-3");
    let other_git = SideRead {
        ahead: vec![],
        ..git.clone()
    };
    let (started, drawing) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let reads = DesignReads::new(Box::new(BlockedRender {
        started,
        release: Mutex::new(released),
    }));
    let (finished, received) = mpsc::channel();
    std::thread::scope(|scope| {
        let slow = scope.spawn(|| reads.of(&lists, "bae", &top, &git));
        drawing.recv_timeout(Duration::from_secs(5)).unwrap();
        let reads = &reads;
        let lists = &lists;
        scope.spawn(move || {
            finished
                .send(reads.of(lists, "bae", &other, &other_git))
                .unwrap();
        });
        let answer = received.recv_timeout(Duration::from_secs(3));
        release.send(()).unwrap();
        assert_eq!(slow.join().unwrap().unwrap()[0].covered_by, Some(id));
        assert!(
            answer
                .expect("the other worktree can answer")
                .unwrap()
                .is_empty()
        );
    });
}
