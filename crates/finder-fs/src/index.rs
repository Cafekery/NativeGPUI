//! The whole-Mac index: every reachable directory read once, kept current by FSEvents, persisted
//! between launches, and answering the questions a listing alone cannot — a folder's recursive
//! size, a name search across the disk, a listing before the disk has been asked.
//!
//! Contract (stub bodies; the real implementation keeps these signatures):
//! - `Index::open` loads the last snapshot from `store` in well under a second for ~2M entries.
//! - `crawl` walks `roots` in order on background threads with `list::list_dir`, skipping what
//!   it cannot read, and reports progress; a second call resumes rather than restarts.
//! - `children`/`folder_size`/`search` answer from memory in milliseconds.
//! - `note_changed` re-reads the named directories (the watcher's job to call).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::entry::Entry;

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    pub current: Option<PathBuf>,
    pub done: bool,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub entry: Entry,
    /// Higher is better: prefix beats word start beats substring; shallower beats deeper.
    pub score: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    pub crawled_at: Option<SystemTime>,
    /// The FSEvents id the snapshot is current to, for catching up on launch.
    pub event_id: u64,
}

pub struct Index {
    _store: PathBuf,
}

impl Index {
    pub fn open(store: &Path) -> Arc<Index> {
        Arc::new(Index {
            _store: store.to_path_buf(),
        })
    }

    pub fn crawl(
        self: &Arc<Self>,
        _roots: Vec<PathBuf>,
        _progress: impl Fn(Progress) + Send + 'static,
    ) {
    }

    pub fn children(&self, _dir: &Path) -> Option<Vec<Entry>> {
        None
    }

    pub fn folder_size(&self, _dir: &Path) -> Option<u64> {
        None
    }

    /// Name search, optionally scoped `under` a directory, best hits first.
    pub fn search(&self, _query: &str, _under: Option<&Path>, _limit: usize) -> Vec<Hit> {
        Vec::new()
    }

    pub fn note_changed(&self, _dirs: &[PathBuf]) {}

    pub fn save(&self) {}

    pub fn stats(&self) -> Stats {
        Stats::default()
    }
}
