//! Change notifications: FSEvents streams for the folders on screen, and the catch-up stream the
//! index uses to learn what changed while the app was not running.
//!
//! Contract (stub; the real implementation keeps these signatures): callbacks fire on a
//! background thread with the directories whose contents changed, coalesced; `since` replays
//! everything after `event_id` and then keeps going.

use std::path::{Path, PathBuf};

pub struct Watcher {}

impl Watcher {
    pub fn new(_on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> Watcher {
        Watcher {}
    }

    pub fn watch(&self, _dir: &Path) {}

    pub fn unwatch(&self, _dir: &Path) {}
}

/// Every directory changed after `event_id` under `roots`, then live changes, with the latest
/// event id alongside so the caller can persist where it got to.
pub fn since(
    _event_id: u64,
    _roots: Vec<PathBuf>,
    _on_change: impl Fn(Vec<PathBuf>, u64) + Send + 'static,
) -> Watcher {
    Watcher {}
}

pub fn latest_event_id() -> u64 {
    0
}
