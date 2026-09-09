use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
}

pub fn spawn(index: &Arc<Index>, roots: Vec<PathBuf>, report: impl Fn(Progress) + Send + 'static) {
    let index = index.clone();
    std::thread::spawn(move || {
        let now = watch::latest_event_id();
        let mut roots: Vec<PathBuf> = roots.iter().map(|root| crawl::real(root)).collect();
        roots.dedup();
        let plan = plan(&index.read(), &roots, now);
        if std::env::var_os("NG_CRAWL_TRACE").is_some() {
            eprintln!(
                "catchup warm {:?} cold {:?} since {}",
                plan.warm, plan.cold, plan.since_when
            );
        }
        if plan.warm.is_empty() {
            let walk = Walk {
                crawl: roots.clone(),
                record: roots,
                since: now,
            };
            return crawl::spawn(&index, walk, report);
        }
        let (batch, history_done) = replay(&plan);
        let walk = reconcile(
            &index,
            &plan,
            batch,
            roots,
            if history_done { now } else { plan.since_when },
        );
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
    });
    let deadline = Instant::now() + REPLAY_TIMEOUT;
    let mut batch = Batch::default();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(replay) = rx.recv_timeout(left) else {
            return (batch, false);
        };
        let done = collect(&mut batch, replay);
        if done {
            return (batch, true);
        }
    }
}

fn collect(batch: &mut Batch, replay: Replay) -> bool {
    batch.wrapped |= replay.wrapped;
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
    let mut crawl = plan.cold.clone();
    if batch.wrapped {
        crawl.extend(plan.warm.iter().cloned());
        return Walk {
            crawl,
            record,
            since,
        };
    }
    let home = crawl::home();
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    {
        let tree = index.read();
        for root in &plan.warm {
            if tree.root_state(root).map(|state| state.modified) != Some(crawl::modified(root)) {
                dirs.insert(root.clone());
            }
        }
        for path in batch.rescan {
            if tree.listed(&path) && open_to_us(&path, &home) {
                crawl.push(path);
            }
        }
        dirs.extend(
            batch
                .dirs
                .iter()
                .filter_map(|path| target(&tree, path, &home)),
        );
    }
    for dir in dirs {
        crawl.extend(index.refresh(&dir, &home));
    }
    Walk {
        crawl,
        record,
        since,
    }
}

fn target(tree: &Tree, path: &Path, home: &Path) -> Option<PathBuf> {
    if !open_to_us(path, home) {
        return None;
    }
    if tree.listed(path) {
        return Some(path.to_path_buf());
    }
    let parent = path.parent()?;
    tree.listed(parent).then(|| parent.to_path_buf())
}

/// Reading a TCC-gated folder again unasked puts its prompt back on screen.
pub(super) fn open_to_us(path: &Path, home: &Path) -> bool {
    !path.ancestors().any(|part| crawl::off_limits(part, home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_path_is_never_reopened() {
        let home = PathBuf::from("/Users/tester");
        assert!(open_to_us(&home.join("Projects/thing"), &home));
        assert!(!open_to_us(
            &home.join("Pictures/Library.photoslibrary/db"),
            &home
        ));
        assert!(!open_to_us(&home.join("Desktop/notes"), &home));
        assert!(!open_to_us(&home.join("Library/Caches/x"), &home));
        assert!(!open_to_us(Path::new("/System/Volumes/Data/Users"), &home));
    }

    #[test]
    fn a_home_that_is_not_ours_is_never_opened() {
        let home = PathBuf::from("/Users/tester");
        assert!(!open_to_us(Path::new("/Users/other/Documents"), &home));
        assert!(!open_to_us(
            Path::new("/Users/other/Library/Containers"),
            &home
        ));
        assert!(!open_to_us(Path::new("/Users/other/Music/iTunes"), &home));
        assert!(open_to_us(Path::new("/Users/other/Projects/thing"), &home));
        assert!(open_to_us(Path::new("/Users/other"), &home));
    }

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
                ..Replay::default()
            }
        ));
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
