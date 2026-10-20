use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};
use std::thread;
use std::time::{Duration, Instant};

use finder_sftp::{Error, Lane};
use parking_lot::Mutex;

use super::connected;
use crate::data::Row;

pub(super) const FRESH: Duration = Duration::from_secs(20);
const HELD: usize = 256;
const LISTERS: usize = 2;

pub(crate) type Rows = Arc<Vec<Arc<Row>>>;

struct Listed {
    rows: Rows,
    at: Instant,
}

#[derive(Default)]
struct Queue {
    waiting: HashMap<&'static str, (f32, &'static str)>,
    running: HashSet<&'static str>,
    failed: HashMap<&'static str, Instant>,
    workers: usize,
}

static LISTINGS: LazyLock<Mutex<HashMap<&'static str, Listed>>> = LazyLock::new(Default::default);
static QUEUE: LazyLock<Mutex<Queue>> = LazyLock::new(Default::default);

pub(crate) fn rows(id: &str) -> Option<Rows> {
    LISTINGS.lock().get(id).map(|listed| listed.rows.clone())
}

pub(super) fn cached(id: &str) -> Option<(Rows, Duration)> {
    (LISTINGS.lock().get(id)).map(|listed| (listed.rows.clone(), listed.at.elapsed()))
}

fn young(id: &str) -> bool {
    cached(id).is_some_and(|(_, age)| age < FRESH)
}

/// Whether a read shows the cached rows at once, and whether it lists anyway.
pub(super) fn decide(age: Option<Duration>, held: bool) -> (bool, bool) {
    match age {
        Some(age) if age < FRESH => (true, false),
        Some(_) => (!held, true),
        None => (false, true),
    }
}

pub(super) fn store(id: &'static str, rows: Vec<Arc<Row>>) {
    let mut listings = LISTINGS.lock();
    let listed = Listed {
        rows: Arc::new(rows),
        at: Instant::now(),
    };
    listings.insert(id, listed);
    if listings.len() > HELD {
        let oldest = (listings.iter()).min_by_key(|(_, listed)| listed.at);
        if let Some(oldest) = oldest.map(|(id, _)| *id) {
            listings.remove(oldest);
        }
    }
}

pub(crate) fn forget_listings(under: &str) {
    LISTINGS.lock().retain(|id, _| !id.starts_with(under));
}

pub(super) fn want(scene: &'static str, wants: Vec<(&'static str, f32)>) {
    let wants: Vec<_> = (wants.into_iter()).filter(|(id, _)| !young(id)).collect();
    let mut queue = QUEUE.lock();
    queue.failed.retain(|_, at| at.elapsed() < FRESH);
    queue.waiting.retain(|_, (_, held)| *held != scene);
    for (id, rank) in wants {
        if queue.running.contains(id) || queue.failed.contains_key(id) {
            continue;
        }
        let held = queue.waiting.entry(id).or_insert((rank, scene));
        held.0 = held.0.min(rank);
    }
    while queue.workers < LISTERS && queue.workers < queue.running.len() + queue.waiting.len() {
        queue.workers += 1;
        thread::spawn(work);
    }
}

fn work() {
    loop {
        let mut queue = QUEUE.lock();
        let next = (queue.waiting.iter())
            .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
            .map(|(&id, &(_, scene))| (id, scene));
        let Some((id, scene)) = next else {
            queue.workers -= 1;
            return;
        };
        queue.waiting.remove(id);
        queue.running.insert(id);
        drop(queue);
        let listed = if young(id) {
            Ok(false)
        } else {
            (connected(id).ok_or(Error::Disconnected))
                .and_then(|server| super::rows(&server.server.on(Lane::Listing), id))
                .map(|_| true)
        };
        let mut queue = QUEUE.lock();
        queue.running.remove(id);
        match listed {
            Err(Error::Disconnected) => queue.waiting.retain(|_, (_, held)| *held != scene),
            Err(_) => {
                queue.failed.insert(id, Instant::now());
            }
            Ok(_) => {}
        }
        drop(queue);
        if let Ok(true) = listed {
            super::ahead::replan_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_young_listing_stands_and_an_old_one_shows_only_to_an_empty_window() {
        let young = Some(FRESH / 2);
        let old = Some(FRESH * 2);
        assert_eq!(decide(young, true), (true, false));
        assert_eq!(decide(young, false), (true, false));
        assert_eq!(decide(old, true), (false, true));
        assert_eq!(decide(old, false), (true, true));
        assert_eq!(decide(None, false), (false, true));
    }
}
