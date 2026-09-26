use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use finder_fs::list::Identity;
use gpui::{Context, Window};

use crate::app::FinderDemo;
use crate::data::{self, Column, Line, Row, intern, intern_path};
use crate::listing::{Shape, Walk};
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, LIST_VIEW};
use crate::{icons, index, listing, preview};

const BATCH: usize = 512;
const POLL: Duration = Duration::from_millis(5);
const TICK: Duration = Duration::from_millis(120);
const VOLUMES_EVERY: u64 = 40;

#[derive(Default)]
struct Mail {
    batches: Vec<(Vec<Arc<Row>>, bool)>,
    lines: Option<(bool, Vec<Line>)>,
    listed: bool,
    done: bool,
    trouble: Option<Trouble>,
}

/// Why a folder could not be listed, worked out on the reading thread so the main thread never
/// waits on a disk that stopped answering.
enum Trouble {
    /// Renamed where it stood: a Finder window follows it there.
    Renamed(&'static str),
    /// Deleted, trashed or moved elsewhere: a Finder window falls back to the nearest folder
    /// above it that is left.
    Gone(&'static str),
    Refused(io::Error),
}

type Sorted = (Vec<Arc<Row>>, Option<(bool, Vec<Line>)>);

type Resorted = (HashMap<&'static str, Vec<Arc<Row>>>, Shape, Walk);

/// Each folder as last listed, with its parent, so a rename can be told from a move once the
/// name is gone.
static IDENTITIES: LazyLock<Mutex<HashMap<&'static str, (Identity, Option<Identity>)>>> =
    LazyLock::new(Mutex::default);

fn remember(id: &'static str) {
    let dir = Path::new(id);
    if let Some(found) = finder_fs::list::identity(dir) {
        let parent = dir.parent().and_then(finder_fs::list::identity);
        IDENTITIES.lock().unwrap().insert(id, (found, parent));
    }
}

fn trouble(id: &'static str, error: io::Error) -> Trouble {
    if !matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    ) {
        return Trouble::Refused(error);
    }
    let dir = Path::new(id);
    let was = IDENTITIES.lock().unwrap().get(id).copied();
    let renamed = was.and_then(|(found, parent)| {
        let now = finder_fs::list::path_of(found)?;
        let above = now.parent()?;
        (finder_fs::list::identity(above) == parent).then_some(())?;
        // Renamed where it stood: keep the path the window was given, not the kernel's.
        match dir.parent().and_then(finder_fs::list::real_path).as_deref() == Some(above) {
            true => Some(dir.with_file_name(now.file_name()?)),
            false => Some(now),
        }
    });
    match renamed {
        Some(now) => Trouble::Renamed(intern_path(&now)),
        None => Trouble::Gone(
            dir.ancestors()
                .skip(1)
                .find(|up| finder_fs::list::is_dir(up))
                .map_or("/", intern_path),
        ),
    }
}

/// Ties on the sort column, and a name sort as a whole, are ordered by names ranked once.
fn sort_rows(rows: &mut Vec<Arc<Row>>, (sort, ascending): (Column, bool)) {
    let kinds: Vec<u32> = match sort {
        Column::Kind => {
            let distinct: HashSet<&str> = rows.iter().map(|row| &*row.kind).collect();
            let distinct: Vec<&str> = distinct.into_iter().collect();
            let ranks = finder_fs::sort::ranks(distinct.iter().copied());
            let rank: HashMap<&str, u32> = distinct.into_iter().zip(ranks).collect();
            rows.iter().map(|row| rank[&*row.kind]).collect()
        }
        _ => vec![0; rows.len()],
    };
    let first = folders_first() || sort == Column::Kind;
    let by = |(ka, a): &(u32, Arc<Row>), (kb, b): &(u32, Arc<Row>)| {
        let by = match sort {
            Column::Name => Ordering::Equal,
            Column::Modified => a.modified_at.total_cmp(&b.modified_at),
            Column::Size => a.bytes.cmp(&b.bytes),
            Column::Kind => ka.cmp(kb),
        };
        let folders = match first {
            true => b.discloses.cmp(&a.discloses),
            false => Ordering::Equal,
        };
        folders.then(if ascending { by } else { by.reverse() })
    };
    let mut keyed: Vec<(u32, Arc<Row>)> = kinds.into_iter().zip(rows.drain(..)).collect();
    keyed.sort_by(by);
    for run in keyed.chunk_by_mut(|a, b| by(a, b) == Ordering::Equal) {
        if run.len() < 2 {
            continue;
        }
        let names = finder_fs::sort::ranks(run.iter().map(|(_, row)| &*row.name));
        for ((key, _), name) in run.iter_mut().zip(names) {
            *key = name;
        }
        match sort == Column::Name && !ascending {
            true => run.sort_by_key(|(name, _)| std::cmp::Reverse(*name)),
            false => run.sort_by_key(|(name, _)| *name),
        }
    }
    rows.extend(keyed.into_iter().map(|(_, row)| row));
}

fn folders_first() -> bool {
    static FOLDERS_FIRST: OnceLock<bool> = OnceLock::new();
    *FOLDERS_FIRST.get_or_init(|| {
        std::process::Command::new("defaults")
            .args(["read", "com.apple.finder", "_FXSortFoldersFirst"])
            .output()
            .is_ok_and(|out| out.status.success() && out.stdout.trim_ascii() == b"1")
    })
}

pub(crate) fn spawn_folder_tick(changes: Arc<Mutex<Vec<PathBuf>>>, cx: &mut Context<FinderDemo>) {
    cx.spawn(async move |this, cx| {
        let mut seen = 0;
        let mut seen_tags = 0;
        let mut ticks = 0u64;
        loop {
            cx.background_executor().timer(TICK).await;
            let changed = std::mem::take(&mut *changes.lock().unwrap());
            ticks += 1;
            if ticks.is_multiple_of(VOLUMES_EVERY) {
                cx.background_executor()
                    .spawn(async { data::refresh_volumes() })
                    .detach();
            }
            crate::thumbs::collect_thumbnails();
            crate::thumbs::sweep_thumbnails();
            let landed = crate::thumbs::thumbnails_landed();
            let drawn = std::mem::replace(&mut seen, landed) != landed;
            let tags = crate::tags::tags_landed();
            let tagged = std::mem::replace(&mut seen_tags, tags) != tags;
            if this
                .update(cx, |this, cx| {
                    this.changed(changed, cx);
                    if tagged {
                        this.resync_toolbar();
                    }
                    if drawn || tagged {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

pub(crate) fn spawn_queue_tick(cx: &mut Context<FinderDemo>) {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(TICK).await;
            if this
                .update(cx, |this, cx| {
                    let events = std::mem::take(&mut *this.queue_events.lock().unwrap());
                    this.handle_queue_events(events, cx);
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

impl FinderDemo {
    pub(crate) fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settle_vanished(window, cx);
        for id in self.folders.take_wanted() {
            self.read_folder(id, cx);
        }
        self.sync_stats(cx);
        crate::icons::draw_wanted(&cx.entity(), cx);
        crate::meta::fetch_wanted(&cx.entity(), cx);
        crate::custom::fetch_wanted(&cx.entity(), cx);
        crate::thumbs::frame_drawn();
        // A row asks for its thumbnail as the list lays itself out, which is after this runs.
        let demo = cx.entity();
        cx.defer(move |cx| crate::thumbs::watch_thumbnails(&demo, cx));
        self.follow();
    }

    pub(crate) fn stats_key(&self) -> (u64, usize) {
        (self.selected.rev(), self.focused_lines().len())
    }

    fn sync_stats(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.stats = None;
            self.stats_pending = None;
            self.stats_stale
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        let key = self.stats_key();
        let known = self.stats.as_ref().is_some_and(|stats| stats.key == key);
        if known || self.stats_pending == Some(key) {
            return;
        }
        self.stats_pending = Some(key);
        self.stats_stale
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.stats_stale = Arc::default();
        let stale = self.stats_stale.clone();
        let lines = self.focused_lines();
        let chosen = self.selected.snapshot();
        let tags = self.tags.clone();
        cx.spawn(async move |this, cx| {
            let summary = cx
                .background_executor()
                .spawn(async move { preview::summarize(key, &lines, &chosen, &tags, &stale) })
                .await;
            let Some(stats) = summary else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.stats_pending == Some(key) {
                    this.stats_pending = None;
                }
                this.stats = Some(stats);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn read_folder(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if !id.starts_with('/') || !self.folders.start(id) {
            return;
        }
        let fresh = !self.folders.has(id);
        if fresh {
            self.folders.deliver(id, Vec::new(), true);
        }
        let shown = self
            .folders
            .get(id)
            .is_some_and(|folder| folder.rows.is_empty());
        let mail = Arc::new(Mutex::new(Mail::default()));
        let filling = mail.clone();
        let sorted_by = (self.sort, self.ascending);
        let art = self.art_size();
        let plain = self.plain();
        let executor = cx.background_executor().clone();
        let read = cx.background_executor().spawn(async move {
            let dir = Path::new(id);
            let index = index();
            let folders = index.folders_in(dir);
            let mut all: Vec<Arc<Row>> = Vec::new();
            let known = shown
                .then(|| index.children(dir))
                .flatten()
                .filter(|known| !known.is_empty());
            let indexed = known.is_some();
            if let Some(known) = known {
                let mut rows = data::rows_of(id, &known, &folders);
                sort_rows(&mut rows, sorted_by);
                icons::prepare(&rows, art, &executor);
                let lines = plain.map(|hidden| (hidden, listing::lines_of(&rows, hidden)));
                let mut mail = filling.lock().unwrap();
                mail.batches.push((rows, true));
                mail.lines = lines;
            }
            let post = |mut rows: Vec<Arc<Row>>| {
                let replace = !filling.lock().unwrap().listed;
                if replace && shown && !indexed {
                    sort_rows(&mut rows, sorted_by);
                    icons::prepare(&rows, art, &executor);
                }
                let mut mail = filling.lock().unwrap();
                mail.listed = true;
                mail.batches.push((rows, replace));
            };
            let read = finder_fs::list::list_dir_batched(dir, BATCH, |entries| {
                let rows = data::rows_of(id, &entries, &folders);
                all.extend(rows.iter().cloned());
                post(rows);
            });
            if let Err(error) = read {
                let trouble = trouble(id, error);
                let mut mail = filling.lock().unwrap();
                mail.trouble = Some(trouble);
                mail.done = true;
                return;
            }
            remember(id);
            if finder_fs::icloud::is_drive(dir) {
                let apps = data::icloud_rows(index);
                all.extend(apps.iter().cloned());
                post(apps);
            }
            data::weigh_packages(&mut all);
            sort_rows(&mut all, sorted_by);
            icons::prepare(&all, art, &executor);
            let lines = plain.map(|hidden| (hidden, listing::lines_of(&all, hidden)));
            let mut mail = filling.lock().unwrap();
            mail.batches.push((all, true));
            mail.lines = lines;
            mail.done = true;
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let (batches, lines, done, trouble) = {
                    let mut mail = mail.lock().unwrap();
                    (
                        std::mem::take(&mut mail.batches),
                        mail.lines.take(),
                        mail.done,
                        mail.trouble.take(),
                    )
                };
                let alive = this
                    .update(cx, |this, cx| match trouble {
                        Some(Trouble::Renamed(now)) => this.folder_renamed(id, now, cx),
                        Some(Trouble::Gone(above)) => this.folder_gone(id, above, cx),
                        Some(Trouble::Refused(error)) => {
                            this.deliver(id, Vec::new(), None, true, sorted_by, cx);
                            if fresh {
                                this.folder_refused(id, &error, cx);
                            }
                        }
                        None => this.deliver(id, batches, lines, done, sorted_by, cx),
                    })
                    .is_ok();
                if done || !alive {
                    break;
                }
            }
            read.await;
        })
        .detach();
    }

    fn deliver(
        &mut self,
        id: &'static str,
        batches: Vec<(Vec<Arc<Row>>, bool)>,
        lines: Option<(bool, Vec<Line>)>,
        done: bool,
        sorted_by: (Column, bool),
        cx: &mut Context<Self>,
    ) {
        let landed = !batches.is_empty();
        for (rows, replace) in batches {
            self.folders.deliver(id, rows, replace);
        }
        if done {
            let owed = self.folders.finish(id);
            self.landed(id);
            if owed {
                self.read_folder(id, cx);
            }
        }
        if done && sorted_by != (self.sort, self.ascending) {
            self.sort_folder(id, (self.sort, self.ascending), cx);
        } else if let Some((hidden, lines)) = lines {
            self.built(id, hidden, lines);
        }
        if done {
            self.expand_landed(id);
            if id == self.here() && self.view == GALLERY_VIEW {
                self.settle();
            }
        }
        let landed_row = self.reveal_next.clone().filter(|key| {
            self.folders
                .get(id)
                .is_some_and(|folder| folder.rows.iter().any(|row| &row.path == key))
        });
        if let Some(key) = landed_row {
            self.reveal_next = None;
            self.reveal(&key);
        }
        if landed || done {
            cx.notify();
        }
    }

    fn sort_folder(&mut self, id: &'static str, by: (Column, bool), cx: &mut Context<Self>) {
        let Some(rows) = self.folders.get(id).map(|folder| folder.rows.clone()) else {
            return;
        };
        let was = rows.len();
        let plain = self.plain().filter(|_| id == self.here());
        cx.spawn(async move |this, cx| {
            let sorted = cx
                .background_executor()
                .spawn(async move {
                    let mut rows = rows;
                    sort_rows(&mut rows, by);
                    let lines = plain.map(|hidden| (hidden, listing::lines_of(&rows, hidden)));
                    (rows, lines)
                })
                .await;
            this.update(cx, |this, cx| {
                this.sorted(id, by, was, sorted, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn sorted(
        &mut self,
        id: &'static str,
        by: (Column, bool),
        was: usize,
        sorted: Sorted,
        cx: &mut Context<Self>,
    ) {
        (self.sort, self.ascending) = by;
        self.resync_toolbar();
        if self
            .folders
            .get(id)
            .is_none_or(|folder| folder.rows.len() != was)
        {
            self.sort_folder(id, by, cx);
            return;
        }
        let (rows, lines) = sorted;
        if let Some(folder) = self.folders.get_mut(id) {
            folder.rows = rows;
        }
        if let Some((hidden, lines)) = lines {
            self.built(id, hidden, lines);
        }
    }

    pub(crate) fn resort(&mut self, by: (Column, bool), cx: &mut Context<Self>) {
        let held = self.folders.held();
        let was: Vec<(&'static str, usize)> =
            held.iter().map(|(id, rows)| (*id, rows.len())).collect();
        let shape = self.shape_sorted(self.view == LIST_VIEW, by);
        cx.spawn(async move |this, cx| {
            let sorted = cx
                .background_executor()
                .spawn(async move {
                    let mut folders: HashMap<&'static str, Vec<Arc<Row>>> =
                        held.into_iter().collect();
                    for rows in folders.values_mut() {
                        sort_rows(rows, by);
                    }
                    let walk = shape.build(&|id| folders.get(id).map(Vec::as_slice));
                    (folders, shape, walk)
                })
                .await;
            this.update(cx, |this, cx| {
                this.resorted(by, &was, sorted, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn resorted(
        &mut self,
        by: (Column, bool),
        was: &[(&'static str, usize)],
        sorted: Resorted,
        cx: &mut Context<Self>,
    ) {
        (self.sort, self.ascending) = by;
        self.resync_toolbar();
        let (mut folders, shape, walk) = sorted;
        let mut moved = false;
        for (id, count) in was {
            let Some(rows) = folders.remove(id) else {
                continue;
            };
            match self.folders.get_mut(id) {
                Some(folder) if folder.rows.len() == *count => folder.rows = rows,
                Some(_) => {
                    moved = true;
                    self.sort_folder(id, by, cx);
                }
                None => moved = true,
            }
        }
        if !moved {
            self.shaped(&shape, walk);
        }
        if let Some(key) = self.cursor.clone() {
            self.reveal(&key);
        }
    }

    fn follow(&mut self) {
        let moved = (self.expanded.rev(), self.here(), self.target(), self.view);
        if self.followed == Some(moved) {
            return;
        }
        self.followed = Some(moved);
        let mut wanted: HashSet<&'static str> = HashSet::from([self.here(), self.target()]);
        wanted.extend(self.expanded.iter().map(|key| intern(key)));
        if self.view == COLUMN_VIEW {
            wanted.extend(self.crumb_ids());
        }
        self.folders.pin(wanted.iter().copied());
        IDENTITIES
            .lock()
            .unwrap()
            .retain(|id, _| wanted.contains(id));
        // An FSEvents stream reports its whole subtree, so only the outermost folders are watched.
        let inside = |id: &str| {
            Path::new(id)
                .ancestors()
                .skip(1)
                .any(|up| up.to_str().is_some_and(|up| wanted.contains(up)))
        };
        let outermost: HashSet<&'static str> =
            wanted.iter().copied().filter(|id| !inside(id)).collect();
        for id in &outermost {
            if self.watching.insert(id) {
                self.watcher.watch(Path::new(id));
            }
        }
        let watcher = &self.watcher;
        self.watching.retain(|id| {
            let keep = outermost.contains(id);
            if !keep {
                watcher.unwatch(Path::new(id));
            }
            keep
        });
    }

    fn changed(&mut self, dirs: Vec<PathBuf>, cx: &mut Context<Self>) {
        if dirs.is_empty() {
            return;
        }
        for dir in &dirs {
            crate::tags::forget_tags_in(dir);
            if let Some(id) = self.folders.known(&dir.to_string_lossy()) {
                self.read_folder(id, cx);
            }
        }
        cx.background_executor()
            .spawn(async move { index().note_changed(&dirs) })
            .detach();
    }
}
