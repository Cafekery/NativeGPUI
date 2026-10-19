use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, mpsc};
use std::time::{Duration, Instant};

use gpui::{App, Entity, Priority, RenderImage, WeakEntity, px, size};
use native_gpui::tokens::{FileType, Symbol, draw_file_icon, generation};
use parking_lot::Mutex;

use crate::app::FinderDemo;
use crate::data::{Line, Row};
use crate::view::LIST_VIEW;

type Key = (FileType<'static>, u32);

const LANES: usize = 4;

const BATCH: usize = 16;
const SLICE: Duration = Duration::from_millis(4);
const KEPT_BYTES: usize = 48 << 20;
const FRESH: u64 = 8;

pub(crate) const FIRST_ROWS: usize = 64;
const PREPARE: Duration = Duration::from_millis(60);

static WAITING: AtomicBool = AtomicBool::new(false);
static KEPT: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(Default::default);
static PREPARED: Mutex<Vec<(Key, Option<Symbol>)>> = Mutex::new(Vec::new());

pub(crate) const STAND_INS: [FileType<'static>; 5] = [
    FileType::Folder,
    FileType::FullFolder,
    FileType::Path("/"),
    FileType::Extension("app"),
    FileType::Uti("public.data"),
];

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        generation: generation(),
        ..State::default()
    });
}

#[derive(Default)]
struct State {
    frame: u64,
    generation: u64,
    held: HashMap<Key, Held>,
    // Owed to `KEPT` once its lock is free: the main thread never waits on a listing's thread.
    kept_cleared: bool,
    kept_changed: Vec<Key>,
    sizes: HashMap<FileType<'static>, Vec<u32>>,
    per_file_bytes: usize,
    asked: HashMap<Key, (u64, usize)>,
    asks: usize,
    waited: u64,
    busy: HashMap<Key, bool>,
    lanes: usize,
    ahead: Ahead,
}

struct Held {
    icon: Option<Symbol>,
    used: u64,
}

#[derive(Default)]
struct Ahead {
    folder: &'static str,
    lines: usize,
    size: u32,
    keys: Vec<Key>,
    at: HashMap<Key, usize>,
    center: usize,
    step: usize,
    seen: u64,
    back: Vec<Key>,
}

struct Job {
    keys: Vec<Key>,
    shown: bool,
    generation: u64,
}

type Drawn = (Vec<(Key, Option<Symbol>)>, Vec<Key>);

fn per_file(kind: FileType<'static>) -> bool {
    matches!(kind, FileType::Path(_) | FileType::FullPath(_))
}

fn bytes_of(icon: &Option<Symbol>) -> usize {
    icon.as_ref()
        .and_then(|icon| icon.image.as_bytes(0))
        .map_or(0, <[u8]>::len)
}

fn draw(key: Key) -> Option<Symbol> {
    draw_file_icon(key.0, px(f32::from_bits(key.1)))
}

static LANDED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn landed() -> u64 {
    LANDED.load(Ordering::Relaxed)
}

pub(crate) fn asks() -> usize {
    STATE.with_borrow(|state| state.asks)
}

pub(crate) fn icon(kind: FileType<'static>, size: f32) -> Option<Symbol> {
    STATE.with_borrow_mut(|state| state.icon((kind, size.to_bits())))
}

pub(crate) fn draw_stand_ins(size: f32) {
    STATE.with_borrow_mut(|state| {
        for kind in STAND_INS {
            let key = (kind, size.to_bits());
            if !state.held.contains_key(&key) {
                state.keep(key, draw(key));
            }
        }
    });
}

pub(crate) fn redraw(changed: impl Fn(&str) -> bool) -> Vec<Arc<RenderImage>> {
    STATE.with_borrow_mut(|state| {
        state
            .held
            .iter_mut()
            .filter(|((kind, _), _)| {
                matches!(kind, FileType::Path(at) | FileType::FullPath(at) if changed(at))
            })
            .filter_map(|(&key, held)| {
                let old = std::mem::replace(&mut held.icon, draw(key));
                state.per_file_bytes = state.per_file_bytes - bytes_of(&old) + bytes_of(&held.icon);
                Some(old?.image)
            })
            .collect()
    })
}

/// Drawing an app's icon checks App Management; every check before TCC answers the first asks again.
pub(crate) fn ask_app_management(apps: impl IntoIterator<Item = PathBuf>) {
    static ASKED: Mutex<Option<Instant>> = Mutex::new(None);
    let mut asked = ASKED.lock();
    if asked.is_some_and(|at| at.elapsed() < Duration::from_secs(60)) {
        return;
    }
    for app in apps {
        finder_fs::list::listed_entry_at(&app).ok();
        *asked = Some(Instant::now());
    }
}

pub(crate) fn installed_apps() -> impl Iterator<Item = PathBuf> {
    let apps = Path::new("/Applications");
    finder_fs::list::list_dir(apps)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.package)
        .map(|entry| apps.join(entry.name))
}

pub(crate) fn prepare(rows: &[Arc<Row>], shown: usize, size: f32) {
    let kinds = rows
        .iter()
        .take(shown)
        .filter_map(|row| crate::art::kind_now(row));
    begin(kinds, size).finish();
}

/// Lanes are threads of their own: the listing's row tasks can hold every executor thread.
pub(crate) struct Drawing {
    keys: Arc<Vec<Key>>,
    next: Arc<AtomicUsize>,
    drawn: mpsc::Sender<()>,
    landed: mpsc::Receiver<()>,
    until: Instant,
}

pub(crate) fn begin(kinds: impl IntoIterator<Item = FileType<'static>>, size: f32) -> Drawing {
    // Resolving a kind can read its file and wait on TCC's prompt: never under the locks below.
    let mut keys: Vec<Key> = Vec::new();
    for key in kinds.into_iter().map(|kind| (kind, size.to_bits())) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    {
        let kept = KEPT.lock();
        let prepared = PREPARED.lock();
        keys.retain(|key| !kept.contains(key) && prepared.iter().all(|(held, _)| held != key));
    }
    let until = Instant::now() + PREPARE;
    let (keys, next) = (Arc::new(keys), Arc::new(AtomicUsize::new(0)));
    let (drawn, landed) = mpsc::channel();
    for _ in 1..LANES.min(keys.len()) {
        let (keys, next, drawn) = (keys.clone(), next.clone(), drawn.clone());
        std::thread::spawn(move || lane(&keys, &next, &drawn, until));
    }
    Drawing {
        keys,
        next,
        drawn,
        landed,
        until,
    }
}

impl Drawing {
    pub(crate) fn finish(self) {
        lane(&self.keys, &self.next, &self.drawn, self.until);
        for _ in 0..self.next.load(Ordering::Relaxed).min(self.keys.len()) {
            let left = self.until.saturating_duration_since(Instant::now());
            if self.landed.recv_timeout(left).is_err() {
                break;
            }
        }
    }
}

fn lane(keys: &[Key], next: &AtomicUsize, drawn: &mpsc::Sender<()>, until: Instant) {
    while Instant::now() < until {
        let Some(key) = keys.get(next.fetch_add(1, Ordering::Relaxed)) else {
            break;
        };
        let icon = draw(*key);
        PREPARED.lock().push((*key, icon));
        drawn.send(()).ok();
    }
}

/// Once per frame, before its rows ask.
pub(crate) fn draw_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let prepared = PREPARED
        .try_lock()
        .map(|mut prepared| std::mem::take(&mut *prepared))
        .unwrap_or_default();
    let dropped = STATE.with_borrow_mut(|state| {
        state.frame += 1;
        let stale = state.frame.saturating_sub(FRESH);
        state.asked.retain(|_, (frame, _)| *frame >= stale);
        let dropped = state.refresh();
        for (key, icon) in prepared {
            if !state.held.contains_key(&key) {
                state.keep(key, icon);
            }
        }
        dropped
    });
    for image in dropped {
        cx.drop_image(image, None);
    }
    start_wanted(demo, cx);
}

pub(crate) fn start_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let demo = demo.downgrade();
    cx.defer(move |cx| start(demo, cx));
}

fn start(demo: WeakEntity<FinderDemo>, cx: &mut App) {
    if let Some(finder) = demo.upgrade() {
        let finder = finder.read(cx);
        let list = finder.view == LIST_VIEW;
        // The size a list settles at, not the one its zoom is passing through.
        let size = if list {
            finder.list_look.icon
        } else {
            finder.art_size()
        };
        let lines = finder.lines(list);
        STATE.with_borrow_mut(|state| state.ahead.follow(finder.here(), &lines, size));
    }
    while let Some(job) = STATE.with_borrow_mut(State::lane) {
        let demo = demo.clone();
        cx.spawn(async move |cx| {
            let mut next = Some(job);
            while let Some(job) = next {
                let priority = if job.shown {
                    Priority::High
                } else {
                    Priority::Low
                };
                let (generation, for_screen) = (job.generation, job.shown);
                let drawn = cx
                    .background_executor()
                    .spawn_with_priority(priority, async move { job.run() })
                    .await;
                let (shown, dropped) =
                    STATE.with_borrow_mut(|state| state.land(drawn, generation, for_screen));
                let alive = demo.update(cx, |_, cx| {
                    for image in dropped {
                        cx.drop_image(image, None);
                    }
                    if shown {
                        cx.notify();
                    }
                });
                next = alive.ok().and_then(|()| {
                    STATE.with_borrow_mut(|state| state.pick(LANES + 1 - state.lanes))
                });
            }
            STATE.with_borrow_mut(|state| state.lanes -= 1);
        })
        .detach();
    }
}

impl Job {
    fn run(self) -> Drawn {
        let until = Instant::now() + SLICE;
        let mut keys = self.keys.into_iter().peekable();
        let mut drawn = Vec::new();
        while let Some(key) = keys.next_if(|_| self.shown || !WAITING.load(Ordering::Relaxed)) {
            drawn.push((key, draw(key)));
            if Instant::now() >= until {
                break;
            }
        }
        (drawn, keys.collect())
    }
}

impl State {
    fn icon(&mut self, key: Key) -> Option<Symbol> {
        let frame = self.frame;
        if per_file(key.0) {
            self.ahead.saw(key, frame);
        }
        if let Some(held) = self.held.get_mut(&key) {
            held.used = frame;
            return held.icon.clone();
        }
        self.asks += 1;
        self.asked.insert(key, (frame, self.asks));
        self.waited = frame;
        if self.busy.get(&key) != Some(&true) {
            WAITING.store(true, Ordering::Relaxed);
        }
        self.nearest(key)
    }

    fn nearest(&self, (kind, at): Key) -> Option<Symbol> {
        let at = f32::from_bits(at);
        self.sizes
            .get(&kind)?
            .iter()
            .filter_map(|drawn| self.held.get(&(kind, *drawn))?.icon.as_ref())
            .min_by(|a, b| {
                (f32::from(a.size.width) - at)
                    .abs()
                    .total_cmp(&(f32::from(b.size.width) - at).abs())
            })
            .map(|icon| Symbol {
                size: size(px(at), px(at)),
                ..icon.clone()
            })
    }

    fn refresh(&mut self) -> Vec<Arc<RenderImage>> {
        let now = generation();
        if now == self.generation {
            return Vec::new();
        }
        self.generation = now;
        self.per_file_bytes = 0;
        self.sizes.clear();
        self.kept_cleared = true;
        self.ahead.restart();
        let dropped = self
            .held
            .drain()
            .filter_map(|(_, held)| Some(held.icon?.image))
            .collect();
        self.sync_kept();
        dropped
    }

    fn lane(&mut self) -> Option<Job> {
        if self.lanes == LANES {
            return None;
        }
        let job = self.pick(LANES - self.lanes)?;
        self.lanes += 1;
        Some(job)
    }

    fn pick(&mut self, free: usize) -> Option<Job> {
        let mut shown: Vec<(Key, (u64, usize))> = self
            .asked
            .iter()
            .filter(|(key, _)| self.busy.get(key) != Some(&true))
            .map(|(&key, &stamp)| (key, stamp))
            .collect();
        let job = if shown.is_empty() {
            self.ahead_job()
        } else {
            shown.sort_unstable_by_key(|(_, (frame, order))| (Reverse(*frame), *order));
            // Dealt like cards: the service renders cold icons in arrival order, top down.
            let keys = shown
                .into_iter()
                .step_by(free)
                .take(BATCH)
                .map(|(key, _)| key);
            Some(Job {
                keys: keys.collect(),
                shown: true,
                generation: self.generation,
            })
        };
        if let Some(job) = &job {
            self.busy
                .extend(job.keys.iter().map(|key| (*key, job.shown)));
        }
        let waiting = self
            .asked
            .keys()
            .any(|key| self.busy.get(key) != Some(&true));
        WAITING.store(waiting, Ordering::Relaxed);
        job
    }

    /// IconServices draws cold icons serially: ahead holds one lane, so a shown row waits one icon at most.
    fn ahead_job(&mut self) -> Option<Job> {
        if self.waited == self.frame
            || self.per_file_bytes >= KEPT_BYTES
            || self.busy.values().any(|shown| !shown)
        {
            return None;
        }
        let mut keys = Vec::new();
        while keys.len() < BATCH {
            let Some(key) = self.ahead.next() else {
                break;
            };
            if !self.held.contains_key(&key) && !self.busy.contains_key(&key) {
                keys.push(key);
            }
        }
        (!keys.is_empty()).then_some(Job {
            keys,
            shown: false,
            generation: self.generation,
        })
    }

    fn land(
        &mut self,
        (drawn, left): Drawn,
        generation: u64,
        for_screen: bool,
    ) -> (bool, Vec<Arc<RenderImage>>) {
        let mut shown = false;
        for (key, icon) in drawn {
            if self.busy.get(&key) == Some(&for_screen) {
                self.busy.remove(&key);
            }
            if generation == self.generation {
                shown |= self.asked.remove(&key).is_some();
                self.keep(key, icon);
            }
        }
        LANDED.fetch_add(u64::from(shown), Ordering::Relaxed);
        for key in left.into_iter().rev() {
            if self.busy.get(&key) == Some(&for_screen) {
                self.busy.remove(&key);
                self.ahead.back.push(key);
            }
        }
        let dropped = if shown && self.per_file_bytes > KEPT_BYTES {
            self.evict()
        } else {
            Vec::new()
        };
        (shown, dropped)
    }

    fn sync_kept(&mut self) {
        let Some(mut kept) = KEPT.try_lock() else {
            return;
        };
        if std::mem::take(&mut self.kept_cleared) {
            kept.clear();
        }
        for key in self.kept_changed.drain(..) {
            if self.held.contains_key(&key) {
                kept.insert(key);
            } else {
                kept.remove(&key);
            }
        }
    }

    fn keep(&mut self, key: Key, icon: Option<Symbol>) {
        if per_file(key.0) {
            self.per_file_bytes += bytes_of(&icon);
        }
        let held = Held {
            icon,
            used: self.frame,
        };
        match self.held.insert(key, held) {
            Some(old) if per_file(key.0) => self.per_file_bytes -= bytes_of(&old.icon),
            Some(_) => {}
            None => self.sizes.entry(key.0).or_default().push(key.1),
        }
        self.kept_changed.push(key);
        self.sync_kept();
    }

    fn evict(&mut self) -> Vec<Arc<RenderImage>> {
        let fresh = self.frame.saturating_sub(FRESH);
        let mut far: Vec<(usize, Key)> = self
            .held
            .iter()
            .filter(|(key, held)| per_file(key.0) && held.used < fresh)
            .map(|(key, _)| (self.ahead.distance(key), *key))
            .collect();
        far.sort_unstable_by_key(|(distance, _)| Reverse(*distance));
        let mut dropped = Vec::new();
        for (_, key) in far {
            if self.per_file_bytes <= KEPT_BYTES / 2 {
                break;
            }
            let Some(held) = self.held.remove(&key) else {
                continue;
            };
            self.kept_changed.push(key);
            self.per_file_bytes -= bytes_of(&held.icon);
            if let Some(sizes) = self.sizes.get_mut(&key.0) {
                sizes.retain(|size| *size != key.1);
                if sizes.is_empty() {
                    self.sizes.remove(&key.0);
                }
            }
            dropped.extend(held.icon.map(|icon| icon.image));
        }
        self.ahead.restart();
        self.sync_kept();
        dropped
    }
}

impl Ahead {
    fn follow(&mut self, folder: &'static str, lines: &Arc<Vec<Line>>, size: f32) {
        let (id, size) = (Arc::as_ptr(lines) as usize, size.to_bits());
        if id == self.lines && size == self.size {
            return;
        }
        if folder != self.folder {
            self.folder = folder;
            self.center = 0;
        }
        self.lines = id;
        self.size = size;
        self.keys.clear();
        self.at.clear();
        for kind in lines.iter().filter_map(|line| line.row.artwork) {
            let key = (kind, size);
            if !self.at.contains_key(&key) {
                self.at.insert(key, self.keys.len());
                self.keys.push(key);
            }
        }
        self.center = self.center.min(self.keys.len().saturating_sub(1));
        self.restart();
    }

    fn restart(&mut self) {
        self.step = 0;
        self.back.clear();
    }

    fn saw(&mut self, key: Key, frame: u64) {
        if self.seen == frame {
            return;
        }
        if let Some(&at) = self.at.get(&key) {
            self.seen = frame;
            if at != self.center {
                self.center = at;
                self.restart();
            }
        }
    }

    fn distance(&self, key: &Key) -> usize {
        self.at
            .get(key)
            .map_or(usize::MAX, |at| at.abs_diff(self.center))
    }

    fn next(&mut self) -> Option<Key> {
        if let Some(key) = self.back.pop() {
            return Some(key);
        }
        let reach = self.center.max(self.keys.len().saturating_sub(self.center));
        while self.step / 2 <= reach {
            let step = self.step;
            self.step += 1;
            let at = match step % 2 {
                0 => self.center.checked_add(step / 2),
                _ => self.center.checked_sub(step / 2 + 1),
            };
            if let Some(key) = at.and_then(|at| self.keys.get(at)) {
                return Some(*key);
            }
        }
        None
    }
}
