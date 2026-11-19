//! The lists as the page reads them: the store's folder followed live, and
//! the lists JSON built from what was read and each
//! running item's worktree.
//!
//! `Feed` follows `Places::lists` with an FSEvents stream and, after the
//! settle delay, asks each store for its last write's version: a store that
//! has not moved is not read again (a hook run's write wakes the feed and
//! reads nothing more), and the reads that found a store changed are counted.
//! Each project's hook runs are read again too. A store that fails its check
//! is served as the JSON's `error`; nothing repairs it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use super::fsevents::Stream;
use super::model::SETTLE;
use crate::lists::DesignChange;
use crate::lists::store::{Loaded, load_all, reload};
use crate::lists::view::{ClaimFacts, DesignView, View, design_view};
use crate::places::Places;

type Snapshot = Result<Vec<Loaded>, String>;

pub struct Feed {
    places: Places,
    /// The version counts reads that changed what is served; the snapshot is
    /// what the last of them read.
    current: Mutex<(u64, Arc<Snapshot>)>,
    /// The hook runs' JSON (hook_stats.rs) as the last read built it.
    hooks: Mutex<Arc<String>>,
    /// The time the hook runs' 12-hour window ends at, in seconds since
    /// 1970; the wall clock, except in a test that sets it.
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    dirty: AtomicBool,
    wake: Condvar,
    wake_lock: Mutex<()>,
    listener: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    stream: Mutex<Option<Stream>>,
}

impl Feed {
    /// Reads the stores and follows their folder. A folder that can't be
    /// made or followed is said on stderr, as the model does for the
    /// checkouts, and the stores are still served as read at start.
    pub fn start(places: Places) -> Arc<Feed> {
        Feed::start_with_clock(places, Box::new(crate::app_timing::now))
    }

    /// `start`, with <clock> telling the time the hook runs' window ends at.
    pub fn start_with_clock(
        places: Places,
        clock: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> Arc<Feed> {
        let dir: PathBuf = places.lists.clone();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("builds: creating {}: {error}", dir.display());
        }
        // Taken before the first read: a write after it is reported by the
        // stream, one before it is in the read.
        let since = Stream::current_event_id();
        let feed = Arc::new(Feed {
            current: Mutex::new((1, Arc::new(load_all(&places)))),
            hooks: Mutex::new(Arc::new(crate::hook_stats::dashboard_json_at(
                &places,
                clock(),
            ))),
            clock,
            places,
            dirty: AtomicBool::new(false),
            wake: Condvar::new(),
            wake_lock: Mutex::new(()),
            listener: Mutex::new(None),
            stream: Mutex::new(None),
        });
        let weak = Arc::downgrade(&feed);
        let started = Stream::start_since(
            since,
            &[dir.as_path()],
            &[],
            0.1,
            Box::new(move |_| {
                if let Some(feed) = weak.upgrade() {
                    let _guard = feed.wake_lock.lock().expect("no panic holding it");
                    feed.dirty.store(true, Ordering::SeqCst);
                    feed.wake.notify_all();
                }
            }),
        );
        *feed.stream.lock().expect("no panic holding it") = Some(started);
        let reader = feed.clone();
        std::thread::Builder::new()
            .name("builds-lists".into())
            .spawn(move || reader.read_loop())
            .expect("a thread starts");
        feed
    }

    /// Calls <listener> after each read that found something new.
    pub fn listen(&self, listener: Box<dyn Fn() + Send + Sync>) {
        *self.listener.lock().expect("no panic holding it") = Some(listener);
    }

    /// The version and what the last read found.
    pub fn current(&self) -> (u64, Arc<Snapshot>) {
        let current = self.current.lock().expect("no panic holding it");
        (current.0, current.1.clone())
    }

    /// The hook runs' JSON as the last read built it. The same `Arc` comes
    /// back until a read finds the runs changed.
    pub fn hooks(&self) -> Arc<String> {
        self.hooks.lock().expect("no panic holding it").clone()
    }

    fn read_loop(self: Arc<Self>) {
        loop {
            {
                let mut guard = self.wake_lock.lock().expect("no panic holding it");
                while !self.dirty.load(Ordering::SeqCst) {
                    guard = self.wake.wait(guard).expect("no panic holding it");
                }
            }
            // Debouncing: let a burst of writes finish.
            std::thread::sleep(SETTLE);
            self.dirty.store(false, Ordering::SeqCst);
            self.read();
        }
    }

    /// Reads the stores whose version moved and the hook runs again; when
    /// either differs from the last read, it calls the listener, and it moves
    /// the version when the stores differ.
    fn read(&self) {
        let previous = self.current.lock().expect("no panic holding it").1.clone();
        let (read, stores_changed) = reload(&self.places, &previous);
        let hooks = crate::hook_stats::dashboard_json_at(&self.places, (self.clock)());
        let mut changed = false;
        if stores_changed {
            let mut current = self.current.lock().expect("no panic holding it");
            *current = (current.0 + 1, Arc::new(read));
            changed = true;
        }
        changed |= replace_if_changed(&self.hooks, hooks);
        if changed && let Some(listener) = &*self.listener.lock().expect("no panic holding it") {
            listener();
        }
    }
}

/// Puts <read> in <held> when it differs from what <held> holds, and returns
/// whether it did. An unchanged value keeps its `Arc`, which tells the page's
/// streams that nothing changed.
fn replace_if_changed(held: &Mutex<Arc<String>>, read: String) -> bool {
    let mut current = held.lock().expect("no panic holding it");
    if **current == read {
        return false;
    }
    *current = Arc::new(read);
    true
}

/// The worktrees of the running items of every project.
pub fn running_worktrees(snapshot: &Snapshot) -> Vec<String> {
    let Ok(stores) = snapshot.as_deref() else {
        return Vec::new();
    };
    stores
        .iter()
        .filter_map(|loaded| loaded.lists.as_ref().ok())
        .flat_map(|lists| &lists.items)
        .flat_map(|item| item.running.iter().map(|r| r.worktree.clone()))
        .collect()
}

/// The items running in <worktree>, in the order the items are listed.
fn items_in<'a>(
    lists: &'a crate::lists::Lists,
    worktree: &'a str,
) -> impl Iterator<Item = &'a crate::lists::Item> {
    lists
        .items
        .iter()
        .filter(move |item| item.running.iter().any(|r| r.worktree == worktree))
}

/// The items running in <worktree> as its card lists them, in the order the
/// items are listed.
pub fn running_items(
    lists: &crate::lists::Lists,
    worktree: &str,
) -> Vec<super::cards::RunningItem> {
    items_in(lists, worktree)
        .map(|item| super::cards::RunningItem {
            id: item.id.0,
            title: item.title.clone(),
        })
        .collect()
}

/// What the running items of <worktree> wait on that is not met, item by
/// item in list order. Fails when a wait names a record the lists do not
/// hold, or one of another kind.
pub fn running_waits(
    lists: &crate::lists::Lists,
    worktree: &str,
) -> Result<Vec<super::cards::ItemWait>, String> {
    let mut found = Vec::new();
    for item in items_in(lists, worktree) {
        found.extend(item_waits(lists, item)?);
    }
    Ok(found)
}

/// What <item> waits on that is not met, in the order of its waits. Fails
/// when a wait names a record the lists do not hold, or one of another kind
/// than the wait says.
pub fn item_waits(
    lists: &crate::lists::Lists,
    item: &crate::lists::Item,
) -> Result<Vec<super::cards::ItemWait>, String> {
    use super::cards::ItemWait;
    use crate::lists::{Kind, Standing, Wait};
    let mut found: Vec<ItemWait> = Vec::new();
    for wait in crate::lists::derive::unmet_waits(lists, item) {
        let (Wait::Question(id) | Wait::Item(id) | Wait::Design(id)) = wait else {
            let Wait::Condition(text) = wait else {
                unreachable!("a wait names a record or a condition")
            };
            found.push(ItemWait::Condition(text.clone()));
            continue;
        };
        let (kind, word) = match wait {
            Wait::Question(_) => (Kind::Question, "question"),
            Wait::Item(_) => (Kind::Item, "item"),
            _ => (Kind::Design, "design change"),
        };
        let located = lists
            .locate(*id)
            .ok_or_else(|| format!("{} waits on {id}, which the lists do not hold", item.id))?;
        if located.kind != kind {
            return Err(format!(
                "{} waits on {id} as a {word}, and {id} is a {}",
                item.id,
                located.kind.name()
            ));
        }
        let title = located.title.to_string();
        found.push(match wait {
            Wait::Question(_) => ItemWait::Answer { id: id.0, title },
            Wait::Design(_) if crate::lists::derive::wait_on_person(lists, wait) => {
                ItemWait::Review { id: id.0, title }
            }
            Wait::Design(_) => ItemWait::Landing { id: id.0, title },
            _ => {
                let state = match located.standing {
                    Standing::Open => {
                        let other = lists.item(*id).ok_or_else(|| {
                            format!("{id} is an open item, which the lists do not hold")
                        })?;
                        crate::lists::derive::item_state(lists, other).label()
                    }
                    Standing::Closed(closed) => closed.how.label(),
                };
                ItemWait::Item {
                    id: id.0,
                    title,
                    state,
                }
            }
        });
    }
    Ok(found)
}

/// Open design change <id> of project <project>, and the change as the
/// lists JSON shows it, which fails on a broken store; None when that
/// project has no open design change with that id.
pub fn open_design(
    snapshot: &Snapshot,
    project: &str,
    id: u32,
) -> Option<(DesignChange, crate::fail::Result<DesignView>)> {
    let loaded = snapshot
        .as_deref()
        .ok()?
        .iter()
        .find(|loaded| loaded.project == project)?;
    let lists = loaded.lists.as_ref().ok()?;
    let design = lists.designs.iter().find(|d| d.id.0 == id)?;
    Some((design.clone(), design_view(lists, design, None)))
}

/// The repo that holds <lists>'s design files: the one repo its design
/// places' globs name. None when no repo has design places; fails when
/// several do, since the Designs page draws one repo's boards.
pub fn design_repo(lists: &crate::lists::Lists) -> crate::fail::Result<Option<&str>> {
    let repos: Vec<&str> = lists
        .design_places
        .iter()
        .filter(|(_, globs)| !globs.is_empty())
        .map(|(repo, _)| repo.as_str())
        .collect();
    match repos[..] {
        [] => Ok(None),
        [repo] => Ok(Some(repo)),
        _ => crate::fail!(
            "project {} keeps design files in {} repos ({}), and the Designs page draws the boards of one",
            lists.project,
            repos.len(),
            repos.join(", ")
        ),
    }
}

/// The repo that holds project <project>'s design files, as `design_repo`
/// finds it; None when the lists hold no such project.
pub fn project_design_repo(
    snapshot: &Snapshot,
    project: &str,
) -> Option<crate::fail::Result<Option<String>>> {
    let loaded = snapshot
        .as_deref()
        .ok()?
        .iter()
        .find(|loaded| loaded.project == project)?;
    Some(match &loaded.lists {
        Ok(lists) => design_repo(lists).map(|repo| repo.map(str::to_string)),
        Err(fault) => Err(crate::fail::Fail::Message(fault.clone())),
    })
}

/// What the page reads: one lists JSON per project, in project order. No
/// project, or a store that fails its check, is that project's `error`.
/// <claims> answers for a running item's worktree.
pub fn compose(
    snapshot: &Snapshot,
    now: i64,
    claims: &dyn Fn(&str) -> ClaimFacts,
) -> Vec<Composed> {
    match snapshot.as_deref() {
        Ok([]) => vec![Composed::of(
            View::failed(
                "",
                0,
                "there are no lists; `builds lists init <project>` makes them".to_string(),
            ),
            false,
        )],
        Ok(stores) => stores
            .iter()
            .map(|loaded| {
                let designs = loaded
                    .lists
                    .as_ref()
                    .is_ok_and(|lists| !matches!(design_repo(lists), Ok(None)));
                Composed::of(compose_view(loaded, now, claims), designs)
            })
            .collect(),
        Err(fault) => vec![Composed::of(View::failed("", 0, fault.clone()), false)],
    }
}

/// The lists JSON of every project, as the page and `/api/lists` have it.
#[derive(serde::Serialize)]
pub struct Served<'a> {
    pub projects: &'a [Composed],
}

/// One project's lists JSON: the view `lists show --json` prints, and the
/// running work the Work tab lists (board.rs), which needs the claim facts
/// only the daemon has.
#[derive(serde::Serialize)]
pub struct Composed {
    #[serde(flatten)]
    pub view: View,
    pub works: Vec<super::board::Work>,
    pub board: super::board::Board,
    /// Whether the project has a Designs page: a repo holds its design files.
    /// The view's `designs` is the list of its design changes. A project whose design files are in several repos has one too, which
    /// says why it draws none (`design_repo`).
    pub has_designs: bool,
}

impl Composed {
    fn of(view: View, has_designs: bool) -> Composed {
        let works = super::board::works(&view);
        let board = super::board::board(&view);
        Composed {
            view,
            works,
            board,
            has_designs,
        }
    }
}

fn compose_view(loaded: &Loaded, now: i64, claims: &dyn Fn(&str) -> ClaimFacts) -> View {
    match &loaded.lists {
        Err(fault) => View::failed(&loaded.project, 0, fault.clone()),
        Ok(lists) => {
            let mut view = match View::build(lists, now) {
                Ok(view) => view,
                Err(fault) => return View::failed(&loaded.project, lists.version, fault.text()),
            };
            for item in &mut view.items {
                for running in &mut item.running {
                    running.claim = Some(claims(&running.worktree));
                }
            }
            view
        }
    }
}

#[cfg(test)]
#[path = "lists_tests.rs"]
mod tests;
