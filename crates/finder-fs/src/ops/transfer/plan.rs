use std::cell::RefCell;
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use objc2_foundation::NSFileManager;

use super::{Gate, Item, Job, PLAN_FOR, Swap, Task, What, copy, volume};
use crate::ops::{
    Conflict, NAME_MAX, deep, duplicate_path, free_path, place_one, real_dir, same_item,
    staging_path, taken,
};

const PATH_ROOM: usize = libc::PATH_MAX as usize - NAME_MAX - 2;

const VOLUME_LITTER: [&[u8]; 5] = [
    b".Spotlight-V100",
    b".fseventsd",
    b".Trashes",
    b".TemporaryItems",
    b".DocumentRevisions-V100",
];

enum Landing {
    Fresh(PathBuf),
    Over(PathBuf),
    Into(PathBuf),
}

struct Walk<'j, 'a> {
    job: &'j Job<'a>,
    outbox: &'j Outbox,
    root: usize,
    source_xattrs: bool,
    clone: bool,
}

impl Walk<'_, '_> {
    fn queue(&self, task: Task) {
        self.outbox.send(self.job, task);
    }
}

/// Holds tasks for `PLAN_FOR`, so a total known not to fit is refused before anything is written.
struct Outbox {
    tasks: Sender<Task>,
    held: RefCell<Option<Vec<Task>>>,
    until: Instant,
}

impl Outbox {
    fn send(&self, job: &Job, task: Task) {
        job.count(&task);
        let mut held = self.held.borrow_mut();
        let Some(waiting) = held.as_mut() else {
            let _ = self.tasks.send(task);
            return;
        };
        waiting.push(task);
        if Instant::now() >= self.until {
            drop(held);
            self.release(job);
        }
    }

    fn release(&self, job: &Job) {
        let Some(held) = self.held.borrow_mut().take() else {
            return;
        };
        if job.fits() {
            for task in held {
                let _ = self.tasks.send(task);
            }
        }
    }

    fn wait_while_paused(&self, job: &Job) {
        if *job.shared.paused.lock() {
            self.release(job);
        }
        job.shared.wait_while_paused();
    }
}

pub(super) fn walk(job: &Job, items: &[Item], tasks: Sender<Task>) {
    let fm = NSFileManager::defaultManager();
    let outbox = Outbox {
        tasks,
        held: RefCell::new(Some(Vec::new())),
        until: Instant::now() + PLAN_FOR,
    };
    for (root, item) in items.iter().enumerate() {
        if root > 0 {
            outbox.wait_while_paused(job);
        }
        if job.halted() {
            return;
        }
        if let Err(error) = plan(job, &fm, root, item, &outbox) {
            job.fail(&item.source, &item.dest, error);
        }
    }
    outbox.release(job);
}

fn plan(
    job: &Job,
    fm: &NSFileManager,
    root: usize,
    item: &Item,
    outbox: &Outbox,
) -> io::Result<()> {
    let meta = fs::symlink_metadata(&item.source)?;
    let into = item.dest.parent().unwrap_or(Path::new("/"));
    let (into_dev, into_gid) = folder(into)?;
    if job.is_move() && meta.dev() == into_dev {
        job.shared.wait_while_paused();
        if job.halted() {
            return Ok(());
        }
        let placed = place_one(fm, &item.source, item.dest.clone(), job.conflict)?;
        if let Some(dest) = &placed.dest {
            job.resolved(root, dest);
        }
        job.placed(root, placed);
        job.count_done(0);
        return Ok(());
    }
    let Some(landing) = resolve(job, &item.source, &item.dest, &meta) else {
        return Ok(());
    };
    let parent = item.source.parent().unwrap_or(Path::new("/"));
    let walk = Walk {
        job,
        outbox,
        root,
        source_xattrs: volume::native_xattrs(parent),
        clone: job.volume.clones && meta.dev() == job.volume.dev,
    };
    let at = match &landing {
        Landing::Fresh(at) | Landing::Over(at) | Landing::Into(at) => at,
    };
    job.resolved(root, at);
    let gate = Gate::open(into_gid);
    place(&walk, &item.source, &meta, landing, &gate, true, true);
    Ok(())
}

fn folder(path: &Path) -> io::Result<(u64, u32)> {
    match fs::metadata(path) {
        Ok(meta) => Ok((meta.dev(), meta.gid())),
        Err(error) if error.raw_os_error() == Some(libc::ENAMETOOLONG) => {
            let meta = deep::stat(path)?;
            Ok((meta.st_dev as u64, meta.st_gid))
        }
        Err(error) => Err(error),
    }
}

fn resolve(job: &Job, source: &Path, dest: &Path, meta: &Metadata) -> Option<Landing> {
    if !taken(dest) {
        return Some(Landing::Fresh(dest.to_path_buf()));
    }
    if same_item(source, dest) {
        return Some(Landing::Fresh(duplicate_path(dest)));
    }
    match job.conflict {
        Conflict::Skip => None,
        Conflict::KeepBoth => Some(Landing::Fresh(free_path(dest))),
        Conflict::Merge if meta.is_dir() && real_dir(dest) => {
            Some(Landing::Into(dest.to_path_buf()))
        }
        Conflict::Replace | Conflict::Merge => Some(Landing::Over(dest.to_path_buf())),
    }
}

fn place(
    walk: &Walk,
    source: &Path,
    meta: &Metadata,
    landing: Landing,
    parent: &Arc<Gate>,
    top: bool,
    is_root: bool,
) {
    let job = walk.job;
    let task = |what: What, dest: PathBuf, size: u64, over: bool| Task {
        what,
        root: walk.root,
        source: source.to_path_buf(),
        dest,
        parent: parent.clone(),
        size,
        over,
        is_root,
        top,
        clone: walk.clone,
    };
    if meta.is_dir() {
        let dest = match landing {
            Landing::Fresh(dest) => dest,
            Landing::Into(dest) => {
                if is_root {
                    job.result(walk.root, &dest);
                }
                let gid = fs::metadata(&dest).map_or(u32::MAX, |meta| meta.gid());
                return children(walk, source, &dest, &Gate::open(gid), true);
            }
            Landing::Over(dest) => {
                let staged = staging_path(&dest);
                job.stage(Swap {
                    root: walk.root,
                    source: source.to_path_buf(),
                    staged: staged.clone(),
                    dest,
                });
                let gate = Gate::pending();
                let mut made = task(What::Dir(gate.clone()), staged.clone(), 0, false);
                (made.is_root, made.top) = (false, false);
                walk.queue(made);
                return children(walk, source, &staged, &gate, false);
            }
        };
        if long(source) || long(&dest) {
            return walk.queue(task(What::Deep, dest, 0, false));
        }
        let gate = Gate::pending();
        walk.queue(task(What::Dir(gate.clone()), dest.clone(), 0, false));
        return children(walk, source, &dest, &gate, false);
    }
    let (dest, over) = match landing {
        Landing::Fresh(dest) => (dest, false),
        Landing::Over(dest) | Landing::Into(dest) => (dest, true),
    };
    if long(source) || long(&dest) {
        return walk.queue(task(What::Deep, dest, 0, over));
    }
    if meta.is_symlink() {
        return walk.queue(task(What::Link, dest, 0, over));
    }
    if !meta.is_file() {
        let error = copy::because(
            job,
            source,
            io::ErrorKind::Unsupported,
            "it isn't a file or a folder",
        );
        return job.fail(source, &dest, error);
    }
    let size = meta.len();
    if size > job.volume.max_file {
        return job.fail(source, &dest, io::Error::from_raw_os_error(libc::EFBIG));
    }
    if over && !is_root && !job.is_move() && identical(meta, &dest) {
        return job.count_done(size);
    }
    walk.queue(task(What::File, dest, size, over));
}

fn identical(source: &Metadata, dest: &Path) -> bool {
    fs::symlink_metadata(dest).is_ok_and(|dest| {
        dest.is_file() && dest.len() == source.len() && dest.mtime().abs_diff(source.mtime()) <= 1
    })
}

fn long(path: &Path) -> bool {
    path.as_os_str().len() > PATH_ROOM
}

fn children(walk: &Walk, source: &Path, dest: &Path, gate: &Arc<Gate>, merging: bool) {
    let job = walk.job;
    let entries = match list(source) {
        Ok(entries) => entries,
        Err(error) => {
            job.unsettled(dest);
            return job.fail(source, dest, error);
        }
    };
    let volume_root = crate::volumes::mount_point(source).is_some_and(|mount| mount == source);
    let mut junk = Vec::new();
    for entry in entries {
        if job.halted() {
            return;
        }
        let (name, meta) = match entry {
            Ok(entry) => entry,
            Err(error) => {
                job.fail(source, dest, error);
                continue;
            }
        };
        let from = source.join(&name);
        if left_behind(walk, name.as_bytes(), volume_root) {
            junk.push(from);
            continue;
        }
        let to = dest.join(&name);
        let landing = match merging && taken(&to) {
            false => Landing::Fresh(to),
            true if meta.is_dir() && real_dir(&to) => Landing::Into(to),
            true => Landing::Over(to),
        };
        place(walk, &from, &meta, landing, gate, merging, false);
    }
    if job.is_move() {
        job.emptied(source, junk);
    }
}

fn left_behind(walk: &Walk, name: &[u8], volume_root: bool) -> bool {
    // Without native attributes a volume already shows a `._` file as its companion's attributes.
    (!walk.source_xattrs && name.starts_with(b"._"))
        || (!walk.job.volume.apple && name == b".DS_Store")
        || (volume_root && VOLUME_LITTER.contains(&name))
}

/// Read whole before descending, so a deep tree holds one directory descriptor at a time.
fn list(dir: &Path) -> io::Result<Vec<io::Result<(OsString, Metadata)>>> {
    Ok(fs::read_dir(dir)?
        .map(|entry| {
            let entry = entry?;
            Ok((entry.file_name(), entry.metadata()?))
        })
        .collect())
}
