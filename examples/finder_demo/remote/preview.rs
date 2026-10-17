use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
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
const SMALL: u32 = 160;
/// An EXIF segment is at most 64 KB, so the thumbnail ends inside this.
const HEAD: u64 = 72 << 10;
const SHOWN: [Face; 9] = [
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Want {
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

static ASKED: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(Default::default);
static QUEUE: Mutex<Vec<(Key, PathBuf)>> = Mutex::new(Vec::new());
static BUSY: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn cached_copy(row: &Row, pixels: u32) -> Option<PathBuf> {
    if !SHOWN.contains(&row.face) {
        return None;
    }
    let whole = cached(&row.path)?;
    if fresh(&whole, row, true) {
        return Some(whole);
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
    if want != Want::Whole && fresh(&local, row, false) {
        return Some(local);
    }
    let key = (row.path.clone(), row.modified_at.to_bits(), want);
    if ASKED.lock().insert(key.clone()) {
        QUEUE.lock().push((key, local));
        if BUSY.fetch_add(1, Ordering::Relaxed) < WORKERS {
            thread::spawn(work);
        } else {
            BUSY.fetch_sub(1, Ordering::Relaxed);
        }
    }
    None
}

pub(crate) fn prune_fetches(drawn: &HashSet<SharedString>) {
    let mut queue = QUEUE.lock();
    let mut asked = ASKED.lock();
    queue.retain(|(key, _)| {
        let kept = drawn.contains(&key.0);
        if !kept {
            asked.remove(key);
        }
        kept
    });
}

fn fresh(local: &Path, row: &Row, whole: bool) -> bool {
    fs::metadata(local).ok().is_some_and(|meta| {
        (!whole || meta.len() == row.bytes)
            && meta.modified().ok().map(reference_seconds) == Some(row.modified_at)
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
        // A `while let` guard would hold the queue lock through the whole download.
        let Some((key, local)) = QUEUE.lock().pop() else {
            break;
        };
        let fetched = connected(&key.0)
            .ok_or(Error::Disconnected)
            .and_then(|server| {
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
                fs::write(&local, bytes)?;
                File::open(&local)?.set_modified(reference_time(f64::from_bits(key.1)))?;
                Ok(())
            });
        match fetched {
            Ok(()) => {
                ASKED.lock().remove(&key);
                crate::thumbs::wake();
            }
            Err(Error::Disconnected) => {
                ASKED.lock().remove(&key);
            }
            Err(_) => {}
        }
    }
    BUSY.fetch_sub(1, Ordering::Relaxed);
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
