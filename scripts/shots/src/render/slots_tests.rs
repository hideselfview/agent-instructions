use super::*;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;

const SLOTS: usize = 2;
const SLOT_COUNT: NonZeroUsize = NonZeroUsize::new(SLOTS).unwrap();
const WORKERS: usize = 5;

const FOLDER: &str = "BUILDS_SHOTS_TEST_SLOT_FOLDER";
const NAME: &str = "BUILDS_SHOTS_TEST_WORKER";

/// The line a worker prints, `HOLDING <name> <pages open>`, when its page is
/// open.
const HOLDING: &str = "HOLDING";

/// One process of the test below: it opens a page of the pinned Chrome, takes
/// its slot from the folder the parent names, prints how many pages the
/// workers have open, and keeps the page open until the parent sends a line.
/// The parent starts it with --ignored.
#[test]
#[ignore = "a process of a_host_draws_no_more_pages_than_it_has_slots"]
fn worker() {
    let folder = PathBuf::from(std::env::var_os(FOLDER).expect("the parent names the folder"));
    let name = std::env::var(NAME).expect("the parent names the worker");
    let open = folder.join("open");
    std::fs::create_dir_all(&open).unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let slots = Slots::new(&folder.join("slots"), SLOT_COUNT);
        crate::render::chrome::with_chrome_slots(
            &crate::render::fixture::cache(),
            slots,
            async |chrome| {
                chrome
                    .with_page(&format!("page of {name}"), async |_page| {
                        std::fs::write(open.join(&name), "").unwrap();
                        let pages = std::fs::read_dir(&open).unwrap().count();
                        println!("{HOLDING} {name} {pages}");
                        std::io::stdout().flush().unwrap();
                        let mut release = String::new();
                        std::io::stdin().read_line(&mut release).unwrap();
                        std::fs::remove_file(open.join(&name)).unwrap();
                        Ok(())
                    })
                    .await
                    .unwrap();
            },
        )
        .await
        .unwrap();
    });
}

enum Said {
    Holding { worker: usize, pages: usize },
    Waiting,
}

/// Sends each line of <from> that <read> keeps to <to>.
fn forward(
    from: impl std::io::Read + Send + 'static,
    to: mpsc::Sender<Said>,
    read: impl Fn(&str) -> Option<Said> + Send + 'static,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(from).lines() {
            if let Some(said) = read(&line.unwrap()) {
                to.send(said).unwrap();
            }
        }
    });
}

#[test]
fn a_host_draws_no_more_pages_than_it_has_slots_however_many_processes_start_pages() {
    let folder = tempfile::tempdir().unwrap();
    let (said, heard) = mpsc::channel();
    let mut workers: Vec<_> = (0..WORKERS)
        .map(|worker| {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "render::slots::tests::worker",
                    "--nocapture",
                ])
                .env(FOLDER, folder.path())
                .env(NAME, worker.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            forward(child.stdout.take().unwrap(), said.clone(), move |line| {
                let mut words = line.split(' ');
                (words.next() == Some(HOLDING)).then(|| Said::Holding {
                    worker,
                    pages: words.nth(1).unwrap().parse().unwrap(),
                })
            });
            forward(child.stderr.take().unwrap(), said.clone(), |line| {
                line.contains("waiting for one of the host's")
                    .then_some(Said::Waiting)
            });
            child
        })
        .collect();
    drop(said);

    // Every process asks for a page at once. The first SLOTS hold theirs while
    // the rest wait; the parent lets the holders go once every other worker
    // has said it waits, and then lets each later holder go as it holds.
    let (mut holders, mut waiting, mut most, mut released) = (Vec::new(), 0, 0, 0);
    while released < WORKERS {
        match heard
            .recv_timeout(std::time::Duration::from_secs(300))
            .expect("every worker says what it does within 5 minutes")
        {
            Said::Waiting => waiting += 1,
            Said::Holding { worker, pages } => {
                assert!(pages <= SLOTS, "{pages} pages open with {SLOTS} slots");
                most = most.max(pages);
                holders.push(worker);
            }
        }
        if (released == 0 && waiting >= WORKERS - SLOTS && holders.len() == SLOTS)
            || released >= SLOTS
        {
            for worker in holders.drain(..) {
                let stdin = workers[worker].stdin.as_mut().unwrap();
                writeln!(stdin, "release").unwrap();
                released += 1;
            }
        }
    }
    assert_eq!(most, SLOTS, "the slots were never all in use at once");
    for worker in &mut workers {
        assert!(worker.wait().unwrap().success());
    }
}

#[tokio::test]
async fn a_slot_is_free_again_once_its_holder_drops_it() {
    let folder = tempfile::tempdir().unwrap();
    let slots = Slots::new(folder.path(), NonZeroUsize::MIN);
    let held = slots.take("the first page").await.unwrap();
    let waiting = tokio::spawn({
        let slots = slots.clone();
        async move { slots.take("the second page").await }
    });
    drop(held);
    waiting.await.unwrap().unwrap();
}
