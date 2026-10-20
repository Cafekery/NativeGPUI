use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Once};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{Context, EntityId};
use parking_lot::{Condvar, Mutex};

use super::jobs::cache_root;
use super::preview::{self, Job, SHOWN, Want};
use crate::app::FinderDemo;
use crate::data::{Face, Line, Row};

const AHEAD_BYTES: u64 = 512 << 20;
const CACHE_CAP: u64 = 4 << 30;
const TRIM_EVERY: Duration = Duration::from_secs(600);
const SETTLE: Duration = Duration::from_millis(50);

#[derive(Clone)]
struct Scene {
    owner: EntityId,
    folder: &'static str,
    lines: Arc<Vec<Line>>,
    seen: Range<usize>,
    chosen: Vec<usize>,
    thumb: u32,
    hero: Option<u32>,
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
    idle: bool,
}

static BOARD: LazyLock<Mutex<Board>> = LazyLock::new(Default::default);
static WAKE: Condvar = Condvar::new();
static PLANNER: Once = Once::new();

impl FinderDemo {
    pub(crate) fn report_scene(
        &self,
        cx: &Context<Self>,
        folder: &'static str,
        lines: &Arc<Vec<Line>>,
        mut seen: impl DoubleEndedIterator<Item = usize>,
        side: f32,
        previews: bool,
    ) {
        let owner = cx.entity_id();
        if !super::is_remote(folder) {
            return leave(owner);
        }
        let first = seen.next();
        let seen = first.map_or(0..0, |first| first..seen.next_back().unwrap_or(first) + 1);
        let cursor = (self.cursor.as_ref()).and_then(|key| self.line_at(lines, key));
        let mut chosen: Vec<usize> = (self.selected.iter())
            .filter_map(|key| self.line_at(lines, key))
            .collect();
        chosen.sort_unstable_by_key(|&at| (Some(at) == cursor, at));
        let pixels = |side: f32| (side * self.scale).round() as u32;
        let scene = Scene {
            owner,
            folder,
            lines: lines.clone(),
            seen,
            chosen,
            thumb: if previews { pixels(side) } else { 0 },
            hero: self.shown_hero.get().map(pixels),
        };
        let mut board = BOARD.lock();
        if (board.scenes.get(&owner)).is_some_and(|held| held.same(&scene)) {
            return;
        }
        board.changed.insert(folder);
        if let Some(was) = board.scenes.insert(owner, scene) {
            board.changed.insert(was.folder);
        }
        PLANNER.call_once(|| {
            thread::spawn(plan);
        });
        WAKE.notify_one();
    }
}

pub(crate) fn leave(owner: EntityId) {
    let mut board = BOARD.lock();
    if let Some(was) = board.scenes.remove(&owner) {
        board.changed.insert(was.folder);
        WAKE.notify_one();
    }
}

pub(super) fn idle() {
    BOARD.lock().idle = true;
    WAKE.notify_one();
}

pub(super) fn replan_all() {
    let mut board = BOARD.lock();
    let folders: Vec<&'static str> = board.scenes.values().map(|scene| scene.folder).collect();
    board.changed.extend(folders);
    WAKE.notify_one();
}

fn plan() {
    let mut headings: HashMap<EntityId, (&'static str, usize, i8)> = HashMap::new();
    let mut trimmed: Option<Instant> = None;
    loop {
        WAKE.wait_while(&mut BOARD.lock(), |board| {
            board.changed.is_empty() && !board.idle
        });
        thread::sleep(SETTLE);
        let (scenes, changed) = {
            let mut board = BOARD.lock();
            board.idle = false;
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
    let mut picks = score(
        lines.len(),
        |at| kind(&lines[at].row),
        &scene.seen,
        &scene.chosen,
        scene.thumb > 0,
        scene.hero.is_some(),
        direction,
    );
    picks.sort_by(|a, b| a.rank.total_cmp(&b.rank));
    let mut spent = 0;
    let mut jobs = Vec::new();
    for pick in picks {
        let row = &lines[pick.at].row;
        let pixels = if pick.hero {
            scene.hero.unwrap_or(u32::MAX)
        } else {
            scene.thumb
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
            jobs.push(preview::job(
                row,
                tier.want,
                tier.local,
                pick.rank,
                Some(scene.folder),
            ));
        }
    }
    jobs
}

#[derive(Clone, Copy, PartialEq)]
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

struct Pick {
    at: usize,
    hero: bool,
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
                hero: false,
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
                hero: true,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selection_leads_then_the_viewport_then_the_scroll_direction() {
        let kind = |at: usize| if at == 7 { Kind::Other } else { Kind::Image };
        let picks = score(12, kind, &(2..6), &[4], true, true, 1);
        let rank = |at: usize, hero: bool| {
            (picks.iter())
                .find(|pick| pick.at == at && pick.hero == hero)
                .map(|pick| pick.rank)
        };
        assert_eq!(
            [2, 3, 4, 5, 6].map(|at| rank(at, true)),
            [1., 0.5, 0., 0.5, 1.].map(Some)
        );
        assert_eq!(rank(7, false), None, "a movie off screen waits");
        assert!(rank(5, false) < rank(9, false) && rank(9, false) < rank(0, false));
        assert!(rank(9, true) < rank(0, true) && rank(11, true).is_some());
        assert!(picks.iter().all(|pick| {
            let near = pick.hero && pick.at.abs_diff(4) <= 2;
            pick.budgeted == !((2..6).contains(&pick.at) || near)
        }));
    }
}
