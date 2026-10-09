use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

use objc2_foundation::NSFileManager;

use super::{Gate, Item, Job, Swap, Task, What, copy, volume};
use crate::ops::{
    Conflict, NAME_MAX, duplicate_path, free_path, place_one, real_dir, same_item, staging_path,
    taken,
};

/// A folder past this length could hold a name that takes its path past `PATH_MAX`.
const PATH_ROOM: usize = libc::PATH_MAX as usize - NAME_MAX - 2;

/// Finder's own bookkeeping at the top of a volume, which a copy of the whole volume leaves out.
const VOLUME_LITTER: [&[u8]; 5] = [
    b".Spotlight-V100",
    b".fseventsd",
    b".Trashes",
    b".TemporaryItems",
    b".DocumentRevisions-V100",
];

enum Landing {
    Fresh(PathBuf),
    /// Something is there and is replaced.
    Over(PathBuf),
    /// A folder is there and this one merges into it.
    Into(PathBuf),
}

/// What every task queued for one top-level item shares.
struct Walk<'j, 'a> {
    job: &'j Job<'a>,
    tasks: &'j Sender<Task>,
    root: usize,
    /// Without native attributes the source shows each file's `._` companion as a file of its own.
    source_xattrs: bool,
    clone: bool,
}

/// Queues every item as it is found, so the copying starts before the counting ends.
pub(super) fn walk(job: &Job, items: &[Item], tasks: Sender<Task>) {
    let fm = NSFileManager::defaultManager();
    for (root, item) in items.iter().enumerate() {
        if root > 0 {
            job.shared.wait_while_paused();
        }
        if job.halted() {
            return;
        }
        if let Err(error) = plan(job, &fm, root, item, &tasks) {
            job.fail(&item.source, &item.dest, error);
        }
    }
}

fn plan(
    job: &Job,
    fm: &NSFileManager,
    root: usize,
    item: &Item,
    tasks: &Sender<Task>,
) -> io::Result<()> {
    let meta = fs::symlink_metadata(&item.source)?;
    let into = item.dest.parent().unwrap_or(Path::new("/"));
    let into_meta = fs::metadata(into)?;
    if job.is_move() && meta.dev() == into_meta.dev() {
        job.shared.wait_while_paused();
        if job.halted() {
            return Ok(());
        }
        let placed = place_one(fm, &item.source, item.dest.clone(), job.conflict, true)?;
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
        tasks,
        root,
        source_xattrs: volume::native_xattrs(parent),
        clone: job.volume.clones && meta.dev() == job.volume.dev,
    };
    let at = match &landing {
        Landing::Fresh(at) | Landing::Over(at) | Landing::Into(at) => at,
    };
    job.resolved(root, at);
    let gate = Gate::open(into_meta.gid());
    place(&walk, &item.source, &meta, landing, &gate, true, true);
    Ok(())
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

/// `top` marks an item that lands where nothing of this transfer was before it.
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
        let (dest, merging) = match landing {
            Landing::Fresh(dest) => (dest, false),
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
        return children(walk, source, &dest, &gate, merging);
    }
    let (dest, over) = match landing {
        Landing::Fresh(dest) => (dest, false),
        Landing::Over(dest) | Landing::Into(dest) => (dest, true),
    };
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

impl Walk<'_, '_> {
    fn queue(&self, task: Task) {
        self.job.count(&task);
        let _ = self.tasks.send(task);
    }
}

/// Inside a merge a file that matches by size and date is taken as already there, as `rsync` does.
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

/// Junk a copy never carries: `._` companions the source volume already shows as attributes,
/// Finder's `.DS_Store` where Finder does not keep it, and a volume's own bookkeeping.
fn left_behind(walk: &Walk, name: &[u8], volume_root: bool) -> bool {
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
