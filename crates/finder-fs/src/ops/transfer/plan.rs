use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use objc2_foundation::NSFileManager;

use super::{Gate, Item, Job, PLAN_FOR, Swap, Task, What, copy, volume};
use crate::ops::deep::{self, Stat};
use crate::ops::{
    Conflict, clean_in, duplicate_path, free_path, land, next_free, place_one, real_dir,
    replace_slot, same_item, taken,
};

const VOLUME_LITTER: [&[u8]; 5] = [
    b".Spotlight-V100",
    b".fseventsd",
    b".Trashes",
    b".TemporaryItems",
    b".DocumentRevisions-V100",
];

enum Landing {
    Fresh(PathBuf),
    /// Made while planning, so a job planned at the same moment cannot pick the same name.
    Made(PathBuf),
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
        if !job.fits() {
            return job.unmake();
        }
        for task in held {
            let _ = self.tasks.send(task);
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
    let mut cleaned = None;
    for (root, item) in items.iter().enumerate() {
        if root > 0 {
            outbox.wait_while_paused(job);
        }
        if job.halted() {
            break;
        }
        if let Some(into) = item.dest.parent().filter(|into| Some(*into) != cleaned) {
            clean_in(into);
            cleaned = Some(into);
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
    let meta = deep::lstat(&item.source)?;
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
    let landing = match landing {
        Landing::Fresh(at) if meta.is_dir() => {
            let next = || next_free(&item.source, &item.dest);
            let make = |at: &Path| deep::create_dir(at, 0o700);
            let (at, ()) = land(at, next, make)?;
            job.premade(root, &at);
            Landing::Made(at)
        }
        landing => landing,
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
        Landing::Fresh(at) | Landing::Made(at) | Landing::Over(at) | Landing::Into(at) => at,
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

fn resolve(job: &Job, source: &Path, dest: &Path, meta: &Stat) -> Option<Landing> {
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
    meta: &Stat,
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
            Landing::Made(dest) => {
                let gate = Gate::open(parent.gid());
                job.made(&task(What::Dir(gate.clone()), dest.clone(), 0, false));
                job.count_done(0);
                return children(walk, source, &dest, &gate, false);
            }
            Landing::Into(dest) => {
                if is_root {
                    job.result(walk.root, &dest);
                }
                let gid = deep::lstat(&dest).map_or(u32::MAX, |meta| meta.gid());
                return children(walk, source, &dest, &Gate::open(gid), true);
            }
            Landing::Over(dest) => {
                let staged = match replace_slot(&dest) {
                    Ok(staged) => staged,
                    Err(error) => return job.fail(source, &dest, error),
                };
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
        let gate = Gate::pending();
        walk.queue(task(What::Dir(gate.clone()), dest.clone(), 0, false));
        return children(walk, source, &dest, &gate, false);
    }
    let (dest, over) = match landing {
        Landing::Fresh(dest) | Landing::Made(dest) => (dest, false),
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
    if over && !is_root && !job.is_move() && identical(source, meta, &dest) {
        return job.count_done(size);
    }
    walk.queue(task(What::File, dest, size, over));
}

/// Size and date alone match another file written in the same second or unpacked from one archive.
fn identical(source: &Path, meta: &Stat, dest: &Path) -> bool {
    let alike = deep::lstat(dest).is_ok_and(|there| {
        there.is_file()
            && there.len() == meta.len()
            && there.mtime().0.abs_diff(meta.mtime().0) <= 2
    });
    alike && same_bytes(source, dest).unwrap_or(false)
}

fn same_bytes(one: &Path, other: &Path) -> io::Result<bool> {
    let open = |path: &Path| deep::open(path, libc::O_RDONLY, 0);
    let (mut one, mut other) = (open(one)?, open(other)?);
    let (mut here, mut there) = (vec![0; 1 << 20], vec![0; 1 << 20]);
    loop {
        let read = one.read(&mut here)?;
        if read == 0 {
            return Ok(other.read(&mut there)? == 0);
        }
        other.read_exact(&mut there[..read])?;
        if here[..read] != there[..read] {
            return Ok(false);
        }
    }
}

fn children(walk: &Walk, source: &Path, dest: &Path, gate: &Arc<Gate>, merging: bool) {
    let job = walk.job;
    if merging {
        clean_in(dest);
    }
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
            Err((name, error)) => {
                match name {
                    Some(name) => job.fail(&source.join(&name), &dest.join(&name), error),
                    None => job.fail(source, dest, error),
                }
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

/// An entry whose attributes cannot be read keeps its name, so its failure names it.
type Listed = Result<(OsString, Stat), (Option<OsString>, io::Error)>;

fn list(dir: &Path) -> io::Result<Vec<Listed>> {
    Ok(deep::list(dir)?
        .into_iter()
        .map(|(name, meta)| match meta {
            Ok(meta) => Ok((name, meta)),
            Err(error) => Err((Some(name), error)),
        })
        .collect())
}
