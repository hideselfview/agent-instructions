//! The daemon's picture of the build volume: a tree of every entry under the
//! root, kept current from FSEvents, from which a folder's size and newest
//! write are summed in memory.
//!
//! Each entry keeps its name, its place in the tree, its inode and link
//! count, the bytes it takes and its modification time; a regular file of
//! `CLONE_MIN` or more also keeps its physical extents. A file with more than
//! one link keeps its bytes per inode, so a write through one name is seen
//! through every name. A folder's size counts each inode once (rustc
//! hard-links deps/ files into incremental/) and each cloned extent once
//! (cargo uplifts with clonefile), which `du` gets wrong both ways.

use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Condvar, Mutex};

use crate::usage::{BLOCK, CLONE_MIN, Measure, extents};

pub type NodeId = u32;
pub const ROOT: NodeId = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    Other,
    /// A slot on the free list.
    Free,
}

struct Node {
    name: Box<[u8]>,
    parent: NodeId,
    kind: Kind,
    ino: u64,
    nlink: u32,
    /// Allocated bytes; for a file with more than one link, the inode's entry
    /// in `linked` holds them instead.
    bytes: u64,
    mtime: i64,
    /// The change time in nanoseconds, which moves with every write to the
    /// entry's data or metadata.
    changed: i64,
    /// Sorted by name.
    children: Vec<NodeId>,
}

/// What a file with more than one link takes, kept once for all its names.
struct Linked {
    bytes: u64,
    extents: Option<Box<[(i64, u64)]>>,
    names: u32,
}

/// What a walk or an event read about one entry.
pub struct Entry {
    pub name: Box<[u8]>,
    pub kind: KindOf,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u32,
    pub bytes: u64,
    pub mtime: i64,
    pub changed: i64,
    pub extents: Option<Box<[(i64, u64)]>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindOf {
    Dir,
    File,
    Other,
}

impl Entry {
    /// Reads an entry's facts from its metadata, and the extents of a big
    /// regular file from the file at <path>.
    pub fn read(name: &[u8], path: &Path, meta: &Metadata) -> Entry {
        let kind = if meta.is_dir() {
            KindOf::Dir
        } else if meta.is_file() {
            KindOf::File
        } else {
            KindOf::Other
        };
        let bytes = meta.blocks() * 512;
        let extents = if kind == KindOf::File && bytes >= CLONE_MIN {
            extents(path, meta.size()).map(Vec::into_boxed_slice)
        } else {
            None
        };
        Entry {
            name: name.into(),
            kind,
            dev: meta.dev(),
            ino: meta.ino(),
            nlink: meta.nlink() as u32,
            bytes,
            mtime: meta.mtime(),
            changed: change_time(meta),
            extents,
        }
    }
}

pub struct Index {
    pub root: PathBuf,
    pub dev: u64,
    /// The root is the volume's, whose unfollowed entries a walk leaves out;
    /// false for an index of one folder below it.
    volume: bool,
    nodes: Vec<Node>,
    free: Vec<NodeId>,
    linked: HashMap<u64, Linked>,
    /// Extents of big single-link files.
    extents: HashMap<NodeId, Box<[(i64, u64)]>>,
}

fn file_kind(kind: KindOf) -> Kind {
    match kind {
        KindOf::Dir => Kind::Dir,
        KindOf::File => Kind::File,
        KindOf::Other => Kind::Other,
    }
}

impl Index {
    /// An index holding only the volume's <root>, which <meta> describes.
    pub fn new(root: &Path, meta: &Metadata) -> Index {
        Index::rooted(root, meta, true)
    }

    /// An index holding only <folder>, a folder below the volume's root,
    /// which <meta> describes: a walk of it to graft into the volume's.
    pub fn of_folder(folder: &Path, meta: &Metadata) -> Index {
        Index::rooted(folder, meta, false)
    }

    fn rooted(root: &Path, meta: &Metadata, volume: bool) -> Index {
        let entry = Entry::read(b"", root, meta);
        let mut index = Index {
            root: root.to_path_buf(),
            dev: meta.dev(),
            volume,
            nodes: Vec::new(),
            free: Vec::new(),
            linked: HashMap::new(),
            extents: HashMap::new(),
        };
        index.nodes.push(Node {
            name: Box::new([]),
            parent: ROOT,
            kind: Kind::Dir,
            ino: entry.ino,
            nlink: entry.nlink,
            bytes: entry.bytes,
            mtime: entry.mtime,
            changed: entry.changed,
            children: Vec::new(),
        });
        index
    }

    /// The number of entries held.
    pub fn len(&self) -> usize {
        self.nodes.len() - self.free.len()
    }

    fn child(&self, parent: NodeId, name: &[u8]) -> Result<usize, usize> {
        let children = &self.nodes[parent as usize].children;
        children.binary_search_by(|id| self.nodes[*id as usize].name.as_ref().cmp(name))
    }

    /// The components of <path> below the root; None for a path outside it.
    fn components<'a>(&self, path: &'a Path) -> Option<Vec<&'a [u8]>> {
        let rest = path.strip_prefix(&self.root).ok()?;
        let mut parts = Vec::new();
        for component in rest.components() {
            match component {
                Component::Normal(part) => parts.push(part.as_bytes()),
                _ => return None,
            }
        }
        Some(parts)
    }

    /// The entry at <path>, when the index holds it.
    pub fn lookup(&self, path: &Path) -> Option<NodeId> {
        let mut at = ROOT;
        for part in self.components(path)? {
            if self.nodes[at as usize].kind != Kind::Dir {
                return None;
            }
            at = self.nodes[at as usize].children[self.child(at, part).ok()?];
        }
        Some(at)
    }

    /// <parent>'s child <name>, when the index holds it.
    pub fn child_of(&self, parent: NodeId, name: &[u8]) -> Option<NodeId> {
        Some(self.nodes[parent as usize].children[self.child(parent, name).ok()?])
    }

    /// <id>'s children, sorted by name.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id as usize].children
    }

    pub fn name(&self, id: NodeId) -> &[u8] {
        &self.nodes[id as usize].name
    }

    /// Does the index record <parent>'s child <name> as <meta> has it now:
    /// the same inode, its change time the same?
    pub fn unchanged(&self, parent: NodeId, name: &[u8], meta: &Metadata) -> bool {
        self.child_of(parent, name).is_some_and(|id| {
            let node = &self.nodes[id as usize];
            node.ino == meta.ino() && node.changed == change_time(meta)
        })
    }

    pub fn is_dir(&self, id: NodeId) -> bool {
        self.nodes[id as usize].kind == Kind::Dir
    }

    pub fn ino(&self, id: NodeId) -> u64 {
        self.nodes[id as usize].ino
    }

    fn allocate(&mut self, node: Node) -> NodeId {
        match self.free.pop() {
            Some(id) => {
                self.nodes[id as usize] = node;
                id
            }
            None => {
                self.nodes.push(node);
                (self.nodes.len() - 1) as NodeId
            }
        }
    }

    /// Records what <entry> says of the child of <parent> it names, adding it
    /// when missing. A folder that turned into a file, or the reverse, loses
    /// what it held. Returns the child.
    pub fn set(&mut self, parent: NodeId, entry: Entry) -> NodeId {
        let kind = file_kind(entry.kind);
        let id = match self.child(parent, &entry.name) {
            Ok(at) => {
                let id = self.nodes[parent as usize].children[at];
                let was = self.nodes[id as usize].kind;
                if was != kind || self.nodes[id as usize].ino != entry.ino {
                    self.forget_contents(id);
                }
                id
            }
            Err(at) => {
                let id = self.allocate(Node {
                    name: entry.name.clone(),
                    parent,
                    kind,
                    ino: entry.ino,
                    nlink: 1,
                    bytes: 0,
                    mtime: entry.mtime,
                    changed: entry.changed,
                    children: Vec::new(),
                });
                self.nodes[parent as usize].children.insert(at, id);
                id
            }
        };
        let linked_before = self.nodes[id as usize].kind == Kind::File && self.nodes[id as usize].nlink > 1;
        if linked_before && !(kind == Kind::File && entry.nlink > 1) {
            // Its other names went; it holds its bytes itself again.
            self.leave_linked(self.nodes[id as usize].ino);
        }
        let node = &mut self.nodes[id as usize];
        node.kind = kind;
        node.ino = entry.ino;
        node.nlink = entry.nlink;
        node.mtime = entry.mtime;
        node.changed = entry.changed;
        if kind == Kind::File && entry.nlink > 1 {
            node.bytes = 0;
            self.extents.remove(&id);
            let shared = self.linked.entry(entry.ino).or_insert(Linked { bytes: 0, extents: None, names: 0 });
            if !linked_before {
                shared.names += 1;
            }
            shared.bytes = entry.bytes;
            shared.extents = entry.extents;
        } else {
            node.bytes = entry.bytes;
            match entry.extents {
                Some(found) => {
                    self.extents.insert(id, found);
                }
                None => {
                    self.extents.remove(&id);
                }
            }
        }
        id
    }

    /// Drops what <id> holds or shares: its children, its extents, its share
    /// of a linked inode.
    fn forget_contents(&mut self, id: NodeId) {
        let children = std::mem::take(&mut self.nodes[id as usize].children);
        for child in children {
            self.free_subtree(child);
        }
        self.extents.remove(&id);
        let node = &self.nodes[id as usize];
        if node.kind == Kind::File && node.nlink > 1 {
            self.leave_linked(node.ino);
        }
        self.nodes[id as usize].nlink = 1;
    }

    /// One name of linked inode <ino> stops sharing its entry.
    fn leave_linked(&mut self, ino: u64) {
        if let Some(shared) = self.linked.get_mut(&ino) {
            shared.names = shared.names.saturating_sub(1);
            if shared.names == 0 {
                self.linked.remove(&ino);
            }
        }
    }

    /// Records the root's own facts.
    pub fn set_root(&mut self, entry: &Entry) {
        let root = &mut self.nodes[ROOT as usize];
        root.mtime = entry.mtime;
        root.changed = entry.changed;
        root.bytes = entry.bytes;
        root.ino = entry.ino;
    }

    fn free_subtree(&mut self, id: NodeId) {
        let mut pending = vec![id];
        while let Some(at) = pending.pop() {
            self.forget_contents(at);
            pending.extend(std::mem::take(&mut self.nodes[at as usize].children));
            let node = &mut self.nodes[at as usize];
            node.kind = Kind::Free;
            node.name = Box::new([]);
            self.free.push(at);
        }
    }

    /// Takes <id> out of its parent's children, leaving it and what it holds
    /// in place for `attach` or `discard`.
    pub fn detach(&mut self, id: NodeId) {
        let parent = self.nodes[id as usize].parent;
        let name = self.nodes[id as usize].name.clone();
        if let Ok(at) = self.child(parent, &name) {
            self.nodes[parent as usize].children.remove(at);
        }
    }

    /// Puts a detached <id> back as <parent>'s child <name>, replacing what
    /// was there.
    pub fn attach(&mut self, id: NodeId, parent: NodeId, name: &[u8]) {
        match self.child(parent, name) {
            Ok(at) => {
                let old = self.nodes[parent as usize].children[at];
                self.free_subtree(old);
                self.nodes[parent as usize].children[at] = id;
            }
            Err(at) => self.nodes[parent as usize].children.insert(at, id),
        }
        let node = &mut self.nodes[id as usize];
        node.parent = parent;
        node.name = name.into();
    }

    /// Puts what <other>, an index of the folder at <id>, holds in place of
    /// what the folder held here: the folder's own facts and its whole
    /// subtree, links and extents included.
    pub fn graft(&mut self, id: NodeId, other: &Index) {
        self.forget_contents(id);
        let top = &other.nodes[ROOT as usize];
        let node = &mut self.nodes[id as usize];
        node.kind = Kind::Dir;
        node.ino = top.ino;
        node.nlink = top.nlink;
        node.bytes = top.bytes;
        node.mtime = top.mtime;
        node.changed = top.changed;
        let mut pending: Vec<(NodeId, NodeId)> = top.children.iter().map(|child| (*child, id)).collect();
        while let Some((from, parent)) = pending.pop() {
            let entry = other.entry(from);
            let made = self.set(parent, entry);
            pending.extend(other.nodes[from as usize].children.iter().map(|child| (*child, made)));
        }
    }

    /// What <id> records, as the entry a read of it would give.
    fn entry(&self, id: NodeId) -> Entry {
        let node = &self.nodes[id as usize];
        let kind = match node.kind {
            Kind::Dir => KindOf::Dir,
            Kind::File => KindOf::File,
            Kind::Other | Kind::Free => KindOf::Other,
        };
        let (bytes, extents) = match self.linked.get(&node.ino) {
            Some(shared) if node.kind == Kind::File && node.nlink > 1 => (shared.bytes, shared.extents.clone()),
            _ => (node.bytes, self.extents.get(&id).cloned()),
        };
        Entry {
            name: node.name.clone(),
            kind,
            dev: self.dev,
            ino: node.ino,
            nlink: node.nlink,
            bytes,
            mtime: node.mtime,
            changed: node.changed,
            extents,
        }
    }

    /// Frees a detached <id> and what it holds.
    pub fn discard(&mut self, id: NodeId) {
        self.free_subtree(id);
    }

    /// Removes the entry at <id> and everything under it.
    pub fn remove(&mut self, id: NodeId) {
        if id == ROOT {
            return;
        }
        self.detach(id);
        self.free_subtree(id);
    }

    /// Fills in <dir>'s children from a listing of it: every listed entry is
    /// set, and children the listing no longer has go.
    pub fn fill(&mut self, dir: NodeId, entries: Vec<Entry>) -> Vec<NodeId> {
        let listed: HashSet<Box<[u8]>> = entries.iter().map(|e| e.name.clone()).collect();
        let gone: Vec<NodeId> = self.nodes[dir as usize]
            .children
            .iter()
            .copied()
            .filter(|id| !listed.contains(&self.nodes[*id as usize].name))
            .collect();
        for id in gone {
            self.remove(id);
        }
        entries.into_iter().map(|entry| self.set(dir, entry)).collect()
    }

    /// What the subtree at <id> takes and its newest write: each inode and
    /// each cloned extent counted once.
    pub fn measure(&self, id: NodeId) -> Measure {
        let mut inodes = HashSet::new();
        let mut seen_extents = HashSet::new();
        let mut bytes = 0u64;
        let mut newest = i64::MIN;
        let mut pending = vec![id];
        while let Some(at) = pending.pop() {
            let node = &self.nodes[at as usize];
            newest = newest.max(node.mtime);
            match node.kind {
                Kind::Dir => {
                    bytes += node.bytes;
                    pending.extend(node.children.iter().copied());
                }
                Kind::File if node.nlink > 1 => {
                    if !inodes.insert(node.ino) {
                        continue;
                    }
                    if let Some(shared) = self.linked.get(&node.ino) {
                        bytes += match &shared.extents {
                            Some(found) => count_extents(found, &mut seen_extents),
                            None => shared.bytes,
                        };
                    }
                }
                Kind::File => {
                    bytes += match self.extents.get(&at) {
                        Some(found) => count_extents(found, &mut seen_extents),
                        None => node.bytes,
                    };
                }
                Kind::Other => bytes += node.bytes,
                Kind::Free => {}
            }
        }
        Measure { kib: bytes / 1024, newest: Some(newest) }
    }

    /// Bytes of memory the index holds, roughly.
    pub fn footprint(&self) -> usize {
        let names: usize = self.nodes.iter().map(|n| n.name.len()).sum();
        let children: usize = self.nodes.iter().map(|n| n.children.capacity() * 4).sum();
        self.nodes.capacity() * std::mem::size_of::<Node>() + names + children
    }
}

fn count_extents(found: &[(i64, u64)], seen: &mut HashSet<i64>) -> u64 {
    found
        .iter()
        .filter(|(device, _)| seen.insert(*device))
        .map(|(_, bytes)| bytes.div_ceil(BLOCK) * BLOCK)
        .sum()
}

fn change_time(meta: &Metadata) -> i64 {
    meta.ctime() * 1_000_000_000 + meta.ctime_nsec()
}

/// Lists folder <dir>: its entries, each read. Entries gone while listed are
/// left out; None when the folder itself is gone.
pub fn list(dir: &Path) -> std::io::Result<Option<Vec<(Entry, PathBuf)>>> {
    let listed = list_metadata(dir)?;
    Ok(listed.map(|listed| listed.into_iter().map(|one| (Entry::read(&one.name, &one.path, &one.meta), one.path)).collect()))
}

/// One entry of a folder's listing, read no further than its metadata.
pub struct Listed {
    pub name: Box<[u8]>,
    pub path: PathBuf,
    pub meta: Metadata,
}

/// Lists folder <dir> as `list` does, reading each entry's metadata only.
pub fn list_metadata(dir: &Path) -> std::io::Result<Option<Vec<Listed>>> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if gone(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut entries = Vec::new();
    for entry in listing {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if gone(&error) => continue,
            Err(error) => return Err(error),
        };
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if gone(&error) => continue,
            Err(error) => return Err(error),
        };
        entries.push(Listed { name: entry.file_name().as_bytes().into(), path, meta });
    }
    Ok(Some(entries))
}

pub fn gone(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
}

/// Walks the folder at <path>, entry <start> of the index behind <index>,
/// <workers> folders at a time, filling the index as each folder is listed.
/// Folders on another device are kept as entries but not walked. A folder
/// that can't be listed is reported and left empty.
pub fn walk(index: &Mutex<Index>, start: NodeId, path: &Path, workers: usize) {
    struct Queue {
        folders: Vec<(NodeId, PathBuf)>,
        busy: usize,
    }
    let (dev, volume) = {
        let index = index.lock().expect("no panic holding it");
        (index.dev, index.volume)
    };
    let queue = Mutex::new(Queue { folders: vec![(start, path.to_path_buf())], busy: 0 });
    let ready = Condvar::new();
    let work = || {
        loop {
            let (id, folder) = {
                let mut q = queue.lock().expect("no panic holding it");
                loop {
                    if let Some(next) = q.folders.pop() {
                        q.busy += 1;
                        break next;
                    }
                    if q.busy == 0 {
                        ready.notify_all();
                        return;
                    }
                    q = ready.wait(q).expect("no panic holding it");
                }
            };
            let listed = match list(&folder) {
                // The volume root's unfollowed entries stay out.
                Ok(Some(entries)) if id == ROOT && volume => Ok(Some(
                    entries
                        .into_iter()
                        .filter(|(entry, _)| {
                            !std::str::from_utf8(&entry.name).is_ok_and(crate::places::unfollowed)
                        })
                        .collect(),
                )),
                other => other,
            };
            let listed = match listed {
                Ok(listed) => listed,
                Err(error) => {
                    eprintln!("builds: listing {}: {error}", folder.display());
                    Some(Vec::new())
                }
            };
            let mut found = Vec::new();
            if let Some(listed) = listed {
                let (entries, paths): (Vec<Entry>, Vec<PathBuf>) = listed.into_iter().unzip();
                let devs: Vec<bool> = entries.iter().map(|e| e.kind == KindOf::Dir && e.dev == dev).collect();
                let mut index = index.lock().expect("no panic holding it");
                // The folder may have gone from the index since it was queued.
                if index.nodes[id as usize].kind == Kind::Dir {
                    let ids = index.fill(id, entries);
                    for ((child, path), walk) in ids.into_iter().zip(paths).zip(devs) {
                        if walk {
                            found.push((child, path));
                        }
                    }
                }
            }
            let mut q = queue.lock().expect("no panic holding it");
            q.busy -= 1;
            q.folders.extend(found);
            ready.notify_all();
        }
    };
    std::thread::scope(|scope| {
        for _ in 1..workers.max(1) {
            scope.spawn(work);
        }
        work();
    });
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
