use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::thread;

use finder_sftp::{Error, Server};
use gpui::SharedString;
use parking_lot::Mutex;

use super::{connected, jobs::cached, split};
use crate::data::{Face, Row, reference_seconds, reference_time};

fn limit(face: Face) -> u64 {
    match face {
        Face::Image => 256 << 20,
        _ => 64 << 20,
    }
}
const WORKERS: usize = 4;
const WHOLE_LANES: usize = 2;
const LANE_RANK: f32 = 3.;
const SMALL: u32 = 160;
/// An EXIF segment is at most 64 KB, so the thumbnail ends inside this.
const HEAD: u64 = 72 << 10;
pub(super) const SHOWN: [Face; 9] = [
    Face::Image,
    Face::Movie,
    Face::Pdf,
    Face::Text,
    Face::Code,
    Face::Sheet,
    Face::Pages,
    Face::Slides,
    Face::Web,
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Want {
    Whole,
    Small,
    Large,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Embedded {
    Jpeg,
    Raf,
}

type Key = (SharedString, u64, Want);

pub(super) struct Tier {
    pub(super) want: Want,
    local: PathBuf,
    pub(super) ready: bool,
}

pub(super) struct Job {
    key: Key,
    local: PathBuf,
    bytes: u64,
    rank: f32,
    folder: Option<&'static str>,
    order: u64,
}

impl Job {
    fn lane(&self) -> bool {
        self.key.2 == Want::Whole && self.rank >= LANE_RANK
    }
}

struct Running {
    key: Key,
    local: PathBuf,
    lane: bool,
}

#[derive(Default)]
struct Queue {
    waiting: HashMap<Key, Job>,
    running: Vec<Running>,
    failed: HashSet<Key>,
    workers: usize,
    order: u64,
}

impl Queue {
    fn asked(&self, key: &Key) -> bool {
        self.failed.contains(key) || self.running.iter().any(|running| &running.key == key)
    }

    fn staff(&mut self) {
        while self.workers < WORKERS && self.workers < self.running.len() + self.waiting.len() {
            self.workers += 1;
            thread::spawn(work);
        }
    }

    fn take(&mut self) -> Option<(Key, PathBuf, u64)> {
        let full = self.running.iter().filter(|running| running.lane).count() >= WHOLE_LANES;
        let key = (self.waiting.values())
            .filter(|job| !(full && job.lane()))
            .min_by(|a, b| a.rank.total_cmp(&b.rank).then(b.order.cmp(&a.order)))?
            .key
            .clone();
        let job = self.waiting.remove(&key)?;
        self.running.push(Running {
            key: job.key.clone(),
            local: job.local.clone(),
            lane: job.lane(),
        });
        Some((job.key, job.local, job.bytes))
    }
}

static QUEUE: LazyLock<Mutex<Queue>> = LazyLock::new(Default::default);

pub(super) fn tier(row: &Row, pixels: u32) -> Option<Tier> {
    if !SHOWN.contains(&row.face) {
        return None;
    }
    let whole = cached(&row.path)?;
    if fresh(&whole, row, true) {
        return Some(Tier {
            want: Want::Whole,
            local: whole,
            ready: true,
        });
    }
    let want = match (embedded(&row.path), pixels <= SMALL) {
        (Some(_), true) => Want::Small,
        (Some(Embedded::Raf), false) => Want::Large,
        _ => Want::Whole,
    };
    let local = match want {
        Want::Whole if row.bytes > limit(row.face) => return None,
        Want::Whole => whole,
        Want::Small => sidecar(whole, "small"),
        Want::Large => sidecar(whole, "large"),
    };
    let ready = want != Want::Whole && fresh(&local, row, false);
    Some(Tier { want, local, ready })
}

pub(crate) fn cached_copy(row: &Row, pixels: u32) -> Option<PathBuf> {
    let tier = tier(row, pixels)?;
    if tier.want == Want::Small && embedded(&row.path) == Some(Embedded::Raf) {
        let large = sidecar(cached(&row.path)?, "large");
        if !fresh(&large, row, false) {
            ask(job(row, Want::Large, large, LANE_RANK, None));
        }
    }
    if tier.ready {
        return Some(tier.local);
    }
    ask(job(row, tier.want, tier.local, 0., None));
    None
}

pub(super) fn planned(row: &Row, tier: Tier, rank: f32, folder: &'static str) -> Job {
    job(row, tier.want, tier.local, rank, Some(folder))
}

fn job(row: &Row, want: Want, local: PathBuf, rank: f32, folder: Option<&'static str>) -> Job {
    Job {
        key: (row.path.clone(), row.modified_at.to_bits(), want),
        local,
        bytes: row.bytes,
        rank,
        folder,
        order: 0,
    }
}

fn ask(mut job: Job) {
    let mut queue = QUEUE.lock();
    if let Some(held) = queue.waiting.get_mut(&job.key) {
        held.rank = held.rank.min(job.rank);
        return;
    }
    if queue.asked(&job.key) {
        return;
    }
    queue.order += 1;
    job.order = queue.order;
    queue.waiting.insert(job.key.clone(), job);
    queue.staff();
}

pub(super) fn replan(folder: &'static str, jobs: Vec<Job>) {
    let mut queue = QUEUE.lock();
    queue.waiting.retain(|_, job| job.folder != Some(folder));
    for mut job in jobs {
        if queue.asked(&job.key) {
            continue;
        }
        queue.order += 1;
        let order = queue.order;
        match queue.waiting.entry(job.key.clone()) {
            Entry::Occupied(mut held) => {
                let held = held.get_mut();
                held.rank = held.rank.min(job.rank);
                held.folder = held.folder.or(Some(folder));
            }
            Entry::Vacant(vacant) => {
                job.order = order;
                vacant.insert(job);
            }
        }
    }
    queue.staff();
}

pub(super) fn idle() -> bool {
    let queue = QUEUE.lock();
    queue.waiting.is_empty() && queue.running.is_empty()
}

pub(super) fn fetching() -> Vec<PathBuf> {
    let running: Vec<(SharedString, PathBuf)> = (QUEUE.lock().running.iter())
        .map(|running| (running.key.0.clone(), running.local.clone()))
        .collect();
    (running.into_iter())
        .flat_map(|(path, local)| [Some(local), cached(&path)])
        .flatten()
        .collect()
}

pub(super) fn fresh(local: &Path, row: &Row, whole: bool) -> bool {
    current(local, whole.then_some(row.bytes), row.modified_at)
}

fn current(local: &Path, bytes: Option<u64>, modified: f64) -> bool {
    fs::metadata(local).ok().is_some_and(|meta| {
        bytes.is_none_or(|bytes| meta.len() == bytes)
            && meta.modified().ok().map(reference_seconds) == Some(modified)
    })
}

fn sidecar(whole: PathBuf, tier: &str) -> PathBuf {
    let mut path = whole.into_os_string();
    path.push(format!(".{tier}.jpg"));
    path.into()
}

fn embedded(path: &str) -> Option<Embedded> {
    match Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => Some(Embedded::Jpeg),
        "raf" => Some(Embedded::Raf),
        _ => None,
    }
}

fn work() {
    loop {
        let mut queue = QUEUE.lock();
        let Some((key, local, bytes)) = queue.take() else {
            queue.workers -= 1;
            let idle = queue.workers == 0 && queue.waiting.is_empty();
            drop(queue);
            if idle {
                super::ahead::idle();
            }
            return;
        };
        drop(queue);
        let modified = f64::from_bits(key.1);
        let whole = key.2 == Want::Whole;
        let fetched = if current(&local, whole.then_some(bytes), modified) {
            Ok(false)
        } else {
            fetch(&key, &local).map(|()| true)
        };
        let mut queue = QUEUE.lock();
        queue.running.retain(|running| running.key != key);
        match fetched {
            Ok(_) | Err(Error::Disconnected) => {}
            Err(_) => {
                queue.failed.insert(key);
            }
        }
        drop(queue);
        if let Ok(true) = fetched {
            crate::thumbs::wake();
        }
    }
}

fn fetch(key: &Key, local: &Path) -> Result<(), Error> {
    let server = connected(&key.0).ok_or(Error::Disconnected)?;
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent)?;
    }
    let remote = split(&key.0).1;
    let embedded = match key.2 {
        Want::Whole => None,
        want => preview(&server.server, remote, want).ok(),
    };
    let Some(bytes) = embedded else {
        let whole = cached(&key.0).ok_or(Error::Disconnected)?;
        return server.server.download(remote, &whole, &mut |_, _| true);
    };
    fs::write(local, bytes)?;
    File::open(local)?.set_modified(reference_time(f64::from_bits(key.1)))?;
    Ok(())
}

fn preview(server: &Server, remote: &str, want: Want) -> Result<Vec<u8>, Error> {
    let bad = || Error::Protocol(format!("{remote} has no embedded preview"));
    let (start, head) = match embedded(remote) {
        Some(Embedded::Raf) => {
            let (offset, len) = raf_jpeg(&server.read_range(remote, 0, 92)?).ok_or_else(bad)?;
            if want == Want::Large {
                return server.read_range(remote, offset, len);
            }
            (offset, server.read_range(remote, offset, HEAD.min(len))?)
        }
        _ => (0, server.read_range(remote, 0, HEAD)?),
    };
    let (offset, len) = exif_thumbnail(&head).ok_or_else(bad)?;
    match head.get(offset..offset + len) {
        Some(thumb) => Ok(thumb.to_vec()),
        None => server.read_range(remote, start + offset as u64, len as u64),
    }
}

fn raf_jpeg(header: &[u8]) -> Option<(u64, u64)> {
    if !header.starts_with(b"FUJIFILMCCD-RAW ") {
        return None;
    }
    let field =
        |at: usize| Some(u32::from_be_bytes(header.get(at..at + 4)?.try_into().ok()?) as u64);
    Some((field(84)?, field(88)?)).filter(|(_, len)| *len > 0)
}

fn exif_thumbnail(jpeg: &[u8]) -> Option<(usize, usize)> {
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    let tiff = loop {
        let marker = *jpeg.get(at + 1)?;
        let len = u16::from_be_bytes(jpeg.get(at + 2..at + 4)?.try_into().ok()?) as usize;
        if marker == 0xE1 && jpeg.get(at + 4..at + 10)? == b"Exif\0\0" {
            break at + 10;
        }
        if marker == 0xDA {
            return None;
        }
        at += 2 + len;
    };
    let big = match jpeg.get(tiff..tiff + 2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| {
        let bytes: [u8; 2] = jpeg.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        } as usize)
    };
    let u32_at = |at: usize| {
        let bytes: [u8; 4] = jpeg.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        } as usize)
    };
    let ifd0 = tiff + u32_at(tiff + 4)?;
    let ifd1 = tiff + u32_at(ifd0 + 2 + 12 * u16_at(ifd0)?)?;
    let (mut offset, mut len) = (None, None);
    for entry in (0..u16_at(ifd1)?).map(|index| ifd1 + 2 + 12 * index) {
        match u16_at(entry)? {
            0x0201 => offset = Some(tiff + u32_at(entry + 8)?),
            0x0202 => len = Some(u32_at(entry + 8)?),
            _ => {}
        }
    }
    Some((offset?, len?)).filter(|(_, len)| *len > 0)
}
