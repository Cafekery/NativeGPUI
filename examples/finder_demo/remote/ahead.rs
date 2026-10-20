use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Once};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::EntityId;
use parking_lot::Mutex;

use super::jobs::cache_root;
use super::preview::{self, Job, SHOWN, Want};
use crate::app::FinderDemo;
use crate::data::{Face, Line, Row};
use crate::signal::Signal;

const AHEAD_BYTES: u64 = 512 << 20;
const CACHE_CAP: u64 = 4 << 30;
const TRIM_EVERY: Duration = Duration::from_secs(600);
const SETTLE: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub(crate) struct Scene {
    pub(crate) owner: EntityId,
    pub(crate) folder: &'static str,
    pub(crate) lines: Arc<Vec<Line>>,
    pub(crate) seen: Range<usize>,
    pub(crate) chosen: Vec<usize>,
    pub(crate) thumb: u32,
    pub(crate) hero: Option<u32>,
}

impl Scene {
    fn same(&self, other: &Scene) -> bool {
        self.folder == other.folder
            && Arc::ptr_eq(&self.lines, &other.lines)
            && self.seen == other.seen
            && self.chosen == other.chosen
            && self.thumb == other.thumb
            && self.hero == other.hero
    }
}

#[derive(Default)]
struct Board {
    scenes: HashMap<EntityId, Scene>,
    changed: HashSet<&'static str>,
}

static BOARD: LazyLock<Mutex<Board>> = LazyLock::new(Default::default);
static RUNG: Signal = Signal::new();
static PLANNER: Once = Once::new();

pub(crate) fn report(scene: Scene) {
    let mut board = BOARD.lock();
    if (board.scenes.get(&scene.owner)).is_some_and(|held| held.same(&scene)) {
        return;
    }
    board.changed.insert(scene.folder);
    if let Some(was) = board.scenes.insert(scene.owner, scene) {
        board.changed.insert(was.folder);
    }
    drop(board);
    PLANNER.call_once(|| {
        thread::spawn(plan);
    });
    RUNG.ring();
}

pub(crate) fn leave(owner: EntityId) {
    let mut board = BOARD.lock();
    let Some(was) = board.scenes.remove(&owner) else {
        return;
    };
    board.changed.insert(was.folder);
    drop(board);
    RUNG.ring();
}

pub(super) fn idle() {
    RUNG.ring();
}

#[derive(Clone)]
pub(crate) struct Lookout {
    owner: EntityId,
    folder: &'static str,
    lines: Arc<Vec<Line>>,
    chosen: Vec<usize>,
    thumb: u32,
    hero: Rc<Cell<Option<u32>>>,
}

impl Lookout {
    pub(crate) fn saw(&self, seen: Range<usize>) {
        report(Scene {
            owner: self.owner,
            folder: self.folder,
            lines: self.lines.clone(),
            seen,
            chosen: self.chosen.clone(),
            thumb: self.thumb,
            hero: self.hero.get(),
        });
    }
}

impl FinderDemo {
    pub(crate) fn lookout(
        &self,
        owner: EntityId,
        folder: &'static str,
        lines: &Arc<Vec<Line>>,
        side: f32,
        previews: bool,
    ) -> Option<Lookout> {
        if !super::is_remote(folder) {
            return None;
        }
        let mut chosen: Vec<usize> = if self.selected.len() * 16 >= lines.len() {
            (lines.iter().enumerate())
                .filter(|(_, line)| self.selected.contains(&line.key))
                .map(|(at, _)| at)
                .collect()
        } else {
            (self.selected.iter())
                .filter_map(|key| self.line_at(lines, key))
                .collect()
        };
        chosen.sort_unstable();
        let cursor = (self.cursor.as_ref())
            .and_then(|cursor| chosen.iter().position(|&at| &lines[at].key == cursor));
        if let Some(at) = cursor {
            let cursor = chosen.remove(at);
            chosen.push(cursor);
        }
        Some(Lookout {
            owner,
            folder,
            lines: lines.clone(),
            chosen,
            thumb: if previews {
                (side * self.scale).round() as u32
            } else {
                0
            },
            hero: self.hero_pixels.clone(),
        })
    }
}

fn plan() {
    let mut rung = 0;
    let mut headings: HashMap<EntityId, (&'static str, usize, i8)> = HashMap::new();
    let mut trimmed: Option<Instant> = None;
    loop {
        rung = wait(RUNG.after(rung));
        thread::sleep(SETTLE);
        let (scenes, changed) = {
            let mut board = BOARD.lock();
            let changed = std::mem::take(&mut board.changed);
            headings.retain(|owner, _| board.scenes.contains_key(owner));
            let scenes: Vec<Scene> = (board.scenes.values())
                .filter(|scene| changed.contains(scene.folder))
                .cloned()
                .collect();
            (scenes, changed)
        };
        for folder in changed {
            let mut jobs = Vec::new();
            for scene in scenes.iter().filter(|scene| scene.folder == folder) {
                jobs.extend(jobs_for(scene, heading(&mut headings, scene)));
            }
            preview::replan(folder, jobs);
        }
        if preview::idle() && trimmed.is_none_or(|at| at.elapsed() >= TRIM_EVERY) {
            trimmed = Some(Instant::now());
            trim(&cache_root(), CACHE_CAP, &preview::fetching());
        }
    }
}

fn heading(headings: &mut HashMap<EntityId, (&'static str, usize, i8)>, scene: &Scene) -> i8 {
    let start = scene.seen.start;
    let direction = match headings.get(&scene.owner) {
        Some(&(folder, was, direction)) if folder == scene.folder => match start.cmp(&was) {
            Ordering::Greater => 1,
            Ordering::Less => -1,
            Ordering::Equal => direction,
        },
        _ => 0,
    };
    headings.insert(scene.owner, (scene.folder, start, direction));
    direction
}

fn jobs_for(scene: &Scene, direction: i8) -> Vec<Job> {
    let lines = &scene.lines;
    let len = lines.len();
    let seen = scene.seen.start.min(len)..scene.seen.end.min(len);
    let chosen: Vec<usize> = scene
        .chosen
        .iter()
        .copied()
        .filter(|&at| at < len)
        .collect();
    let mut picks = score(
        len,
        |at| kind(&lines[at].row),
        &seen,
        &chosen,
        scene.thumb > 0,
        scene.hero.is_some(),
        direction,
    );
    picks.sort_by(|a, b| a.rank.total_cmp(&b.rank));
    let mut spent = 0;
    let mut jobs = Vec::new();
    for pick in picks {
        let row = &lines[pick.at].row;
        let pixels = match pick.size {
            Size::Thumb => scene.thumb,
            Size::Hero => scene.hero.unwrap_or(u32::MAX),
        };
        let Some(tier) = preview::tier(row, pixels) else {
            continue;
        };
        if pick.budgeted && tier.want != Want::Small {
            if spent >= AHEAD_BYTES {
                continue;
            }
            spent += row.bytes;
        }
        if !tier.ready {
            jobs.push(preview::planned(row, tier, pick.rank, scene.folder));
        }
    }
    jobs
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Skip,
    Image,
    Other,
}

fn kind(row: &Row) -> Kind {
    match row.face {
        _ if row.opens.is_some() => Kind::Skip,
        Face::Image => Kind::Image,
        face if SHOWN.contains(&face) => Kind::Other,
        _ => Kind::Skip,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Size {
    Thumb,
    Hero,
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Pick {
    at: usize,
    size: Size,
    rank: f32,
    budgeted: bool,
}

fn score(
    len: usize,
    kind: impl Fn(usize) -> Kind,
    seen: &Range<usize>,
    chosen: &[usize],
    thumbs: bool,
    hero: bool,
    direction: i8,
) -> Vec<Pick> {
    let focus = chosen
        .last()
        .copied()
        .unwrap_or((seen.start + seen.end) / 2);
    let span = seen.len().max(1) as f32;
    let dist = |at: usize| {
        let dist = at.abs_diff(focus) as f32;
        match (direction, at.cmp(&focus)) {
            (0, _) | (_, Ordering::Equal) => dist,
            (1, Ordering::Greater) | (-1, Ordering::Less) => dist * 0.5,
            _ => dist * 2.,
        }
    };
    let mut sorted = chosen.to_vec();
    sorted.sort_unstable();
    let near = |at: usize| {
        let chosen = |by: usize| {
            [at.checked_sub(by), at.checked_add(by)]
                .into_iter()
                .flatten()
                .any(|at| sorted.binary_search(&at).is_ok())
        };
        (0..3).find(|&by| chosen(by)).map(|by| by as f32 / 2.)
    };
    let mut picks = Vec::new();
    for at in 0..len {
        let kind = kind(at);
        if kind == Kind::Skip {
            continue;
        }
        let inside = seen.contains(&at);
        let dist = dist(at);
        if thumbs && (inside || kind == Kind::Image) {
            picks.push(Pick {
                at,
                size: Size::Thumb,
                rank: if inside { 1. + dist / span } else { 10. + dist },
                budgeted: !inside,
            });
        }
        let rank = near(at)
            .or_else(|| (hero && inside).then_some(3. + dist))
            .or_else(|| (hero && kind == Kind::Image).then_some(20. + dist));
        if let Some(rank) = rank {
            picks.push(Pick {
                at,
                size: Size::Hero,
                rank,
                budgeted: !inside && at.abs_diff(focus) > 2,
            });
        }
    }
    picks
}

fn trim(root: &Path, cap: u64, busy: &[PathBuf]) {
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut total: u64 = files.iter().map(|(_, _, len)| len).sum();
    if total <= cap {
        return;
    }
    files.sort_by_key(|(_, used, _)| *used);
    for (path, _, len) in files {
        if total <= cap {
            break;
        }
        if !busy.contains(&path) && fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

fn walk(dir: &Path, files: &mut Vec<(PathBuf, SystemTime, u64)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(".relay") {
            continue;
        }
        let (Ok(meta), path) = (entry.metadata(), entry.path()) else {
            continue;
        };
        if meta.is_dir() {
            walk(&path, files);
        } else if meta.is_file() {
            let used = meta.accessed().or_else(|_| meta.modified());
            files.push((path, used.unwrap_or(UNIX_EPOCH), meta.len()));
        }
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn wait<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranks(picks: &[Pick], size: Size) -> Vec<(usize, f32)> {
        (picks.iter())
            .filter(|pick| pick.size == size)
            .map(|pick| (pick.at, pick.rank))
            .collect()
    }

    #[test]
    fn the_selection_leads_and_the_scroll_direction_breaks_ties() {
        let kind = |at: usize| if at == 7 { Kind::Other } else { Kind::Image };
        let picks = score(12, kind, &(2..6), &[4], true, false, 1);
        let heroes = ranks(&picks, Size::Hero);
        assert_eq!(heroes, [(2, 1.), (3, 0.5), (4, 0.), (5, 0.5), (6, 1.)]);
        let thumbs = ranks(&picks, Size::Thumb);
        assert!(
            !thumbs.iter().any(|(at, _)| *at == 7),
            "a movie off screen waits"
        );
        let rank = |at: usize| thumbs.iter().find(|(row, _)| *row == at).unwrap().1;
        assert!(
            rank(5) < rank(9) && rank(9) < rank(0),
            "inside, then ahead, then behind"
        );
        assert_eq!(rank(8), 12.);
        assert_eq!(rank(1), 16.);
        assert!(
            (picks.iter())
                .filter(|pick| pick.size == Size::Thumb)
                .all(|pick| pick.budgeted != (2..6).contains(&pick.at))
        );
    }

    #[test]
    fn a_hero_reaches_the_whole_folder() {
        let picks = score(6, |_| Kind::Image, &(0..2), &[], false, true, 0);
        assert!(ranks(&picks, Size::Thumb).is_empty());
        assert_eq!(
            ranks(&picks, Size::Hero),
            [(0, 4.), (1, 3.), (2, 21.), (3, 22.), (4, 23.), (5, 24.)]
        );
    }
}
