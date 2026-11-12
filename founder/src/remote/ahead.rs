use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Once};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use finder_fs::sort::name_cmp;
use gpui::{Context, EntityId};
use parking_lot::{Condvar, Mutex};

use super::jobs::cache_root;
use super::listings;
use super::preview::{self, Job, SHOWN, Want};
use crate::app::FinderDemo;
use crate::data::{Face, Line, Row};
use crate::view::COLUMN_VIEW;

const AHEAD_BYTES: u64 = 512 << 20;
const CACHE_CAP: u64 = 4 << 30;
const ORPHAN_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const TRIM_EVERY: Duration = Duration::from_secs(600);
const SETTLE: Duration = Duration::from_millis(50);
const LIST_AHEAD: usize = 64;
const PEEK: usize = 48;
const PEEK_HEROES: usize = 3;

#[derive(Clone)]
struct Scene {
    owner: EntityId,
    folder: &'static str,
    lines: Arc<Vec<Line>>,
    seen: Range<usize>,
    chosen: Vec<usize>,
    thumb: u32,
    hero: Option<u32>,
    column: bool,
    history: Vec<&'static str>,
}

impl Scene {
    fn same(&self, other: &Scene) -> bool {
        self.folder == other.folder
            && Arc::ptr_eq(&self.lines, &other.lines)
            && self.seen == other.seen
            && self.chosen == other.chosen
            && self.thumb == other.thumb
            && self.hero == other.hero
            && self.column == other.column
            && self.history == other.history
    }
}

#[derive(Default)]
struct Board {
    scenes: HashMap<EntityId, Scene>,
    changed: HashSet<&'static str>,
    woken: bool,
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
        seen: impl Iterator<Item = usize>,
        side: f32,
        previews: bool,
    ) {
        let owner = cx.entity_id();
        if !super::is_remote(folder) {
            return leave(owner);
        }
        let seen = (seen.fold(None, |span: Option<Range<usize>>, at| {
            Some(span.map_or(at..at + 1, |span| span.start.min(at)..span.end.max(at + 1)))
        }))
        .unwrap_or(0..0);
        let cursor = (self.cursor.as_ref()).and_then(|key| self.line_at(lines, key));
        let mut chosen: Vec<usize> = (self.selected.iter())
            .filter_map(|key| self.line_at(lines, key))
            .collect();
        chosen.sort_unstable_by_key(|&at| (Some(at) == cursor, at));
        let pixels = |side: f32| (side * self.scale).round() as u32;
        let history = [self.at.checked_sub(1), Some(self.at + 1)]
            .into_iter()
            .filter_map(|at| self.history.get(at?).copied())
            .filter(|id| super::is_remote(id))
            .collect();
        let scene = Scene {
            owner,
            folder,
            lines: lines.clone(),
            seen,
            chosen,
            thumb: if previews { pixels(side) } else { 0 },
            hero: self.shown_hero.get().map(pixels),
            column: self.view == COLUMN_VIEW,
            history,
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

pub(super) fn replan_all() {
    let mut board = BOARD.lock();
    let folders: Vec<&'static str> = board.scenes.values().map(|scene| scene.folder).collect();
    board.changed.extend(folders);
    board.woken = true;
    WAKE.notify_one();
}

fn plan() {
    let mut headings: HashMap<EntityId, (&'static str, usize, i8)> = HashMap::new();
    let mut trimmed: Option<Instant> = None;
    sweep(&cache_root(), SystemTime::now() - ORPHAN_AGE);
    loop {
        WAKE.wait_while(&mut BOARD.lock(), |board| {
            board.changed.is_empty() && !board.woken
        });
        thread::sleep(SETTLE);
        let (scenes, changed) = {
            let mut board = BOARD.lock();
            board.woken = false;
            let changed = std::mem::take(&mut board.changed);
            headings.retain(|owner, _| board.scenes.contains_key(owner));
            let scenes: Vec<Scene> = (board.scenes.values())
                .filter(|scene| changed.contains(scene.folder))
                .cloned()
                .collect();
            (scenes, changed)
        };
        for folder in changed {
            let (mut jobs, mut peeks, mut lists) = (Vec::new(), Vec::new(), Vec::new());
            for scene in scenes.iter().filter(|scene| scene.folder == folder) {
                let planned = plan_scene(scene, heading(&mut headings, scene));
                jobs.extend(planned.0);
                peeks.extend(planned.1);
                lists.extend(planned.2);
            }
            listings::want(folder, lists);
            if !preview::replan(folder, jobs) {
                preview::replan(folder, peeks);
            }
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

type Wants = Vec<(&'static str, f32)>;

fn plan_scene(scene: &Scene, direction: i8) -> (Vec<Job>, Vec<Job>, Wants) {
    let lines = &scene.lines;
    let focus = Focus::new(scene.seen.clone(), &scene.chosen, direction);
    let mut picks = score(
        lines.len(),
        |at| kind(&lines[at]),
        &focus,
        scene.thumb > 0,
        scene.hero.is_some(),
        scene.column,
    );
    picks.sort_by(|a, b| a.rank.total_cmp(&b.rank));
    let mut spent = 0;
    let mut jobs = Vec::new();
    let mut opens = Vec::new();
    for pick in picks {
        let row = &lines[pick.at].row;
        let pixels = match pick.get {
            Get::List => {
                opens.extend(row.opens.map(|id| (id, pick.rank)));
                continue;
            }
            Get::Thumb => scene.thumb,
            Get::Hero => scene.hero.unwrap_or(u32::MAX),
        };
        jobs.extend(job(
            row,
            pixels,
            pick.rank,
            pick.budgeted,
            scene.folder,
            &mut spent,
        ));
    }
    let parent = Some(super::parent(scene.folder)).filter(|id| super::is_remote(id));
    let lists = list_ahead(opens, &scene.history, parent);
    let mut peeks = Vec::new();
    for &(id, first) in lists.iter().filter(|(_, rank)| *rank < 10.) {
        let Some(rows) = listings::rows(id) else {
            continue;
        };
        let mut images: Vec<&Arc<Row>> = (rows.iter())
            .filter(|row| row.face == Face::Image && !row.hidden)
            .collect();
        images.sort_by(|a, b| name_cmp(&a.name, &b.name));
        for (order, row) in images.into_iter().take(PEEK).enumerate() {
            let rank = 30. + first + order as f32;
            if scene.thumb > 0 {
                peeks.extend(job(row, scene.thumb, rank, true, scene.folder, &mut spent));
            }
            if let Some(hero) = scene.hero.filter(|_| first == 0. && order < PEEK_HEROES) {
                peeks.extend(job(row, hero, rank, true, scene.folder, &mut spent));
            }
        }
    }
    (jobs, peeks, lists)
}

fn job(
    row: &Row,
    pixels: u32,
    rank: f32,
    budgeted: bool,
    folder: &'static str,
    spent: &mut u64,
) -> Option<Job> {
    let tier = preview::tier(row, pixels)?;
    if budgeted && tier.want != Want::Small {
        if *spent >= AHEAD_BYTES {
            return None;
        }
        *spent += row.bytes;
    }
    (!tier.ready).then(|| preview::job(row, tier.want, tier.local, rank, Some(folder)))
}

fn list_ahead(mut wants: Wants, history: &[&'static str], parent: Option<&'static str>) -> Wants {
    wants.extend(history.iter().map(|&id| (id, 1.5)));
    wants.extend(parent.map(|id| (id, 2.)));
    wants.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut seen = HashSet::new();
    wants.retain(|&(id, rank)| seen.insert(id) && (rank < 10. || seen.len() <= LIST_AHEAD));
    wants
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Skip,
    Image,
    Other,
    Folder { nested: bool },
}

fn kind(line: &Line) -> Kind {
    match line.row.face {
        _ if line.row.opens.is_some() => Kind::Folder {
            nested: line.depth > 0,
        },
        Face::Image => Kind::Image,
        face if SHOWN.contains(&face) => Kind::Other,
        _ => Kind::Skip,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Get {
    Thumb,
    Hero,
    List,
}

struct Pick {
    at: usize,
    get: Get,
    rank: f32,
    budgeted: bool,
}

struct Focus {
    seen: Range<usize>,
    at: usize,
    direction: i8,
    chosen: Vec<usize>,
}

impl Focus {
    fn new(seen: Range<usize>, chosen: &[usize], direction: i8) -> Focus {
        let at = (chosen.last().copied()).unwrap_or((seen.start + seen.end) / 2);
        let mut chosen = chosen.to_vec();
        chosen.sort_unstable();
        Focus {
            seen,
            at,
            direction,
            chosen,
        }
    }

    fn dist(&self, at: usize) -> f32 {
        let dist = at.abs_diff(self.at) as f32;
        match (self.direction, at.cmp(&self.at)) {
            (0, _) | (_, Ordering::Equal) => dist,
            (1, Ordering::Greater) | (-1, Ordering::Less) => dist * 0.5,
            _ => dist * 2.,
        }
    }

    fn near(&self, at: usize, reach: usize) -> Option<f32> {
        let chosen = |by: usize| {
            [at.checked_sub(by), at.checked_add(by)]
                .into_iter()
                .flatten()
                .any(|at| self.chosen.binary_search(&at).is_ok())
        };
        (0..=reach).find(|&by| chosen(by)).map(|by| by as f32 / 2.)
    }
}

fn score(
    len: usize,
    kind: impl Fn(usize) -> Kind,
    focus: &Focus,
    thumbs: bool,
    hero: bool,
    column: bool,
) -> Vec<Pick> {
    let span = focus.seen.len().max(1) as f32;
    let mut picks = Vec::new();
    for at in 0..len {
        let kind = kind(at);
        let inside = focus.seen.contains(&at);
        let dist = focus.dist(at);
        match kind {
            Kind::Skip => continue,
            Kind::Folder { nested } => {
                let rest = if nested { 12. } else { 10. };
                let rank = (focus.near(at, if column { 2 } else { 0 }))
                    .or_else(|| inside.then(|| 2. + dist / span))
                    .unwrap_or(rest + dist);
                picks.push(Pick {
                    at,
                    get: Get::List,
                    rank,
                    budgeted: false,
                });
                continue;
            }
            Kind::Image | Kind::Other => {}
        }
        if thumbs && (inside || kind == Kind::Image) {
            picks.push(Pick {
                at,
                get: Get::Thumb,
                rank: if inside { 1. + dist / span } else { 10. + dist },
                budgeted: !inside,
            });
        }
        let rank = (focus.near(at, 2))
            .or_else(|| (hero && inside).then_some(3. + dist))
            .or_else(|| (hero && kind == Kind::Image).then_some(20. + dist));
        if let Some(rank) = rank {
            picks.push(Pick {
                at,
                get: Get::Hero,
                rank,
                budgeted: !inside && at.abs_diff(focus.at) > 2,
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

fn orphan(name: &str) -> bool {
    name.starts_with(".relay") || name.contains(".part.")
}

/// Temp files a crash left behind; `trim` skips them, so nothing else would ever remove them.
fn sweep(dir: &Path, before: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            sweep(&entry.path(), before);
        } else if orphan(&entry.file_name().to_string_lossy())
            && meta.modified().is_ok_and(|modified| modified < before)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn walk(dir: &Path, files: &mut Vec<(PathBuf, SystemTime, u64)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if orphan(&name) {
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
    use crate::data::intern;

    #[test]
    fn a_sweep_removes_only_old_temp_files() {
        let root = std::env::temp_dir().join(format!("founder-sweep-{}", std::process::id()));
        let inner = root.join("host/dir");
        fs::create_dir_all(&inner).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
        let make = |path: PathBuf, modified: SystemTime| {
            let file = fs::File::create(&path).unwrap();
            file.set_modified(modified).unwrap();
            path
        };
        let stale = [
            make(root.join(".relay 3"), old),
            make(inner.join("a.png.part.7"), old),
        ];
        let kept = [
            make(inner.join("a.png"), old),
            make(inner.join("b.png.part.1"), SystemTime::now()),
        ];
        sweep(&root, SystemTime::now() - ORPHAN_AGE);
        assert!(stale.iter().all(|path| !path.exists()));
        assert!(kept.iter().all(|path| path.exists()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_selection_leads_then_the_viewport_then_the_scroll_direction() {
        let kind = |at: usize| if at == 7 { Kind::Other } else { Kind::Image };
        let picks = score(12, kind, &Focus::new(2..6, &[4], 1), true, true, false);
        let rank = |at: usize, get: Get| {
            (picks.iter())
                .find(|pick| pick.at == at && pick.get == get)
                .map(|pick| pick.rank)
        };
        assert_eq!(
            [2, 3, 4, 5, 6].map(|at| rank(at, Get::Hero)),
            [1., 0.5, 0., 0.5, 1.].map(Some)
        );
        assert_eq!(rank(7, Get::Thumb), None, "a movie off screen waits");
        assert!(rank(5, Get::Thumb) < rank(9, Get::Thumb));
        assert!(rank(9, Get::Thumb) < rank(0, Get::Thumb));
        assert!(rank(9, Get::Hero) < rank(0, Get::Hero) && rank(11, Get::Hero).is_some());
        assert!(picks.iter().all(|pick| {
            let near = pick.get == Get::Hero && pick.at.abs_diff(4) <= 2;
            pick.budgeted == !((2..6).contains(&pick.at) || near)
        }));
    }

    #[test]
    fn the_chosen_folder_lists_first_then_history_parent_and_viewport_within_the_cap() {
        let kind = |at: usize| match at {
            150.. => Kind::Image,
            at => Kind::Folder { nested: at >= 100 },
        };
        let ids: Vec<&'static str> = (0..200)
            .map(|at| intern(&format!("sftp://h/{at}")))
            .collect();
        let lists = |column: bool| {
            let picks = score(200, kind, &Focus::new(0..8, &[4], 0), false, false, column);
            assert!(picks.iter().all(|pick| pick.get == Get::List));
            let opens = picks.iter().map(|pick| (ids[pick.at], pick.rank)).collect();
            list_ahead(opens, &[ids[120], "sftp://h/next"], Some("sftp://h/up"))
        };
        let column = lists(true);
        let rank = |id: &str| column.iter().find(|held| held.0 == id).map(|held| held.1);
        assert_eq!(column[0], (ids[4], 0.));
        assert_eq!(
            [3, 5, 2, 6].map(|at| rank(ids[at])),
            [0.5, 0.5, 1., 1.].map(Some)
        );
        assert_eq!(
            rank(ids[120]),
            Some(1.5),
            "a history neighbour outranks its row"
        );
        assert_eq!(rank("sftp://h/next"), Some(1.5));
        assert_eq!(rank("sftp://h/up"), Some(2.));
        assert_eq!([0, 7].map(|at| rank(ids[at])), [2.5, 2.375].map(Some));
        assert_eq!(rank(ids[8]), Some(14.));
        assert_eq!(column.len(), LIST_AHEAD);
        assert_eq!(column.iter().filter(|held| held.0 == ids[120]).count(), 1);
        assert_eq!(
            lists(false)
                .iter()
                .find(|held| held.0 == ids[3])
                .map(|held| held.1),
            Some(2.125)
        );
        let nested = score(200, kind, &Focus::new(0..8, &[4], 0), false, false, false);
        assert_eq!(
            nested
                .iter()
                .find(|pick| pick.at == 101)
                .map(|pick| pick.rank),
            Some(12. + 97.)
        );
    }
}
