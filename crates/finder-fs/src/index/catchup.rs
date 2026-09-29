use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use super::crawl::{self, Walk};
use super::store::Tree;
use super::{Index, Progress};
use crate::watch::{self, Replay};

const REPLAY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub warm: Vec<PathBuf>,
    pub cold: Vec<PathBuf>,
    pub since_when: u64,
}

#[derive(Default)]
struct Batch {
    dirs: BTreeSet<PathBuf>,
    rescan: BTreeSet<PathBuf>,
    wrapped: bool,
    reached: u64,
}

pub fn spawn(index: &Arc<Index>, roots: Vec<PathBuf>, report: impl Fn(Progress) + Send + 'static) {
    let index = index.clone();
    std::thread::spawn(move || {
        crate::utility_qos();
        let now = watch::latest_event_id();
        let mut roots: Vec<PathBuf> = roots.iter().map(|root| crawl::real(root)).collect();
        roots.dedup();
        let plan = plan(&index.read(), &roots, now);
        trace!(
            "catchup warm {:?} cold {:?} since {}",
            plan.warm, plan.cold, plan.since_when
        );
        let whole = match index.owner {
            true => roots,
            false => Vec::new(),
        };
        if plan.warm.is_empty() {
            let walk = Walk {
                crawl: whole.clone(),
                record: whole,
                since: now,
            };
            return crawl::spawn(&index, walk, report);
        }
        let (mut batch, history_done) = replay(&plan);
        let since = match history_done {
            true => now,
            // A replay cut short lost the rest of its events, as dropped ones are lost.
            false => {
                batch.rescan.extend(plan.warm.iter().cloned());
                batch.reached.max(plan.since_when)
            }
        };
        let walk = reconcile(&index, &plan, batch, whole, since);
        crawl::spawn(&index, walk, report);
    });
}

pub(super) fn plan(tree: &Tree, roots: &[PathBuf], now: u64) -> Plan {
    let mut plan = Plan {
        since_when: u64::MAX,
        ..Plan::default()
    };
    for root in roots {
        let state = tree
            .root_state(root)
            .filter(|state| state.event_id != 0 && state.event_id <= now);
        match state.filter(|_| tree.listed(root)) {
            Some(state) => {
                plan.since_when = plan.since_when.min(state.event_id);
                plan.warm.push(root.clone());
            }
            None => plan.cold.push(root.clone()),
        }
    }
    if plan.warm.is_empty() {
        plan.since_when = 0;
    }
    plan
}

fn replay(plan: &Plan) -> (Batch, bool) {
    let (tx, rx) = mpsc::channel();
    let _watcher = watch::since(plan.since_when, plan.warm.clone(), move |replay| {
        let _ = tx.send(replay);
        false
    });
    let deadline = Instant::now() + REPLAY_TIMEOUT;
    let mut batch = Batch::default();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(replay) = rx.recv_timeout(left) else {
            return (batch, false);
        };
        if collect(&mut batch, replay) {
            return (batch, true);
        }
    }
}

fn collect(batch: &mut Batch, replay: Replay) -> bool {
    batch.wrapped |= replay.wrapped;
    batch.reached = batch.reached.max(replay.event_id);
    batch.dirs.extend(replay.dirs);
    batch.rescan.extend(replay.rescan);
    replay.history_done
}

fn reconcile(
    index: &Arc<Index>,
    plan: &Plan,
    batch: Batch,
    record: Vec<PathBuf>,
    since: u64,
) -> Walk {
    let owner = index.owner;
    let mut crawl = match owner {
        true => plan.cold.clone(),
        false => Vec::new(),
    };
    if batch.wrapped {
        if owner {
            crawl.extend(plan.warm.iter().cloned());
        }
        return Walk {
            crawl,
            record,
            since,
        };
    }
    let home = super::home();
    let (dirs, rescans) = {
        let tree = index.read();
        let dirs: BTreeSet<PathBuf> = plan
            .warm
            .iter()
            .filter(|root| {
                tree.root_state(root).map(|state| state.modified) != Some(crawl::modified(root))
            })
            .cloned()
            .chain(
                batch
                    .dirs
                    .iter()
                    .filter_map(|path| target(&tree, path, &home)),
            )
            .collect();
        let rescans: Vec<PathBuf> = batch
            .rescan
            .into_iter()
            .filter(|path| tree.listed(path) && crawl::open_to_us(path, &home))
            .collect();
        (dirs, rescans)
    };
    for dir in dirs {
        crawl.extend(index.refresh(&dir, &home));
    }
    let saved = index.saved.map_or(UNIX_EPOCH, |at| at - super::SAVE_GAP);
    crawl.extend(index.verify(&rescans, saved, &home));
    Walk {
        crawl,
        record,
        since,
    }
}

fn target(tree: &Tree, path: &Path, home: &Path) -> Option<PathBuf> {
    if !crawl::open_to_us(path, home) {
        return None;
    }
    if tree.listed(path) {
        return Some(path.to_path_buf());
    }
    let parent = path.parent()?;
    tree.listed(parent).then(|| parent.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_ids_take_every_root() {
        let plan = Plan {
            warm: vec![PathBuf::from("/a")],
            cold: vec![PathBuf::from("/b")],
            since_when: 7,
        };
        let mut batch = Batch::default();
        assert!(!collect(
            &mut batch,
            Replay {
                dirs: vec![PathBuf::from("/a/x")],
                event_id: 12,
                ..Replay::default()
            }
        ));
        assert_eq!(batch.reached, 12);
        assert!(collect(
            &mut batch,
            Replay {
                wrapped: true,
                history_done: true,
                ..Replay::default()
            }
        ));
        assert!(batch.wrapped);
        let index = Index::open(Path::new("/nonexistent/finder-fs-catchup/index.bin"));
        let walk = reconcile(&index, &plan, batch, Vec::new(), 9);
        assert_eq!(walk.crawl, [PathBuf::from("/b"), PathBuf::from("/a")]);
    }
}
