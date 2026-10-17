use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use finder_sftp::Error;
use gpui::SharedString;
use parking_lot::Mutex;

use super::{connected, jobs::cached, split};
use crate::data::{Face, Row, reference_seconds};

/// Larger files stay icons until opened.
const LIMIT: u64 = 64 << 20;
const WORKERS: usize = 4;
/// Kinds Quick Look renders from their contents; the rest get generic icons without a download.
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

type Key = (SharedString, u64);

static ASKED: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(Default::default);
static QUEUE: Mutex<Vec<(Key, PathBuf)>> = Mutex::new(Vec::new());
static BUSY: AtomicUsize = AtomicUsize::new(0);

/// The cached copy the thumbnail service can read, fetched in the background when missing or stale.
pub(crate) fn cached_copy(row: &Row) -> Option<PathBuf> {
    if !SHOWN.contains(&row.face) || row.bytes > LIMIT {
        return None;
    }
    let local = cached(&row.path)?;
    let fresh = fs::metadata(&local).ok().is_some_and(|meta| {
        meta.len() == row.bytes
            && meta.modified().ok().map(reference_seconds) == Some(row.modified_at)
    });
    if fresh {
        return Some(local);
    }
    let key = (row.path.clone(), row.modified_at.to_bits());
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

/// Drops queued fetches for rows no longer on screen; running ones finish.
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
                server
                    .server
                    .download(split(&key.0).1, &local, &mut |_, _| true)
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
