//! Reading folders off the disk: what a frame asked for, what the watcher says changed, and the
//! order Finder sorts a listing in.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{Context, Window};

use crate::app::FinderDemo;
use crate::data::{self, Column, Line, Row, intern, intern_path};
use crate::listing::{Shape, Walk};
use crate::view::{COLUMN_VIEW, LIST_VIEW};
use crate::{icons, index, listing, preview};

const BATCH: usize = 512;

const POLL: Duration = Duration::from_millis(5);

const TICK: Duration = Duration::from_millis(120);

const VOLUMES_EVERY: u64 = 40;

/// The rows arriving from one directory read, and whether they replace what is on screen.
#[derive(Default)]
struct Mail {
    batches: Vec<(Vec<Arc<Row>>, bool)>,
    /// The sorted listing as lines, and whether hidden rows are in it.
    lines: Option<(bool, Vec<Line>)>,
    listed: bool,
    done: bool,
    /// The read failed because the folder is no longer there.
    gone: bool,
}

/// A folder put in order off the main thread: its rows, and the lines built from them where the
/// listing is drawn as it is read.
type Sorted = (Vec<Arc<Row>>, Option<(bool, Vec<Line>)>);

/// Every listing put in order off the main thread, under the shape the one on screen was built in.
type Resorted = (HashMap<&'static str, Vec<Arc<Row>>>, Shape, Walk);

/// Whether a failed read means the folder itself is gone: deleted, replaced by a file, or on a
/// volume that went away.
fn gone(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

fn order(sort: Column, ascending: bool, a: &Row, b: &Row) -> Ordering {
    // Finder groups folders first under the preference, and always in the Kind column.
    if folders_first() || sort == Column::Kind {
        let by_kind = b.discloses.cmp(&a.discloses);
        if by_kind != Ordering::Equal {
            return by_kind;
        }
    }
    let by = match sort {
        Column::Name => Ordering::Equal,
        Column::Modified => a.modified_at.total_cmp(&b.modified_at),
        Column::Size => a.bytes.cmp(&b.bytes),
        Column::Kind => finder_fs::sort::name_cmp(&a.kind, &b.kind),
    };
    let by = by.then_with(|| finder_fs::sort::name_cmp(&a.name, &b.name));
    if ascending { by } else { by.reverse() }
}

/// Finder's "keep folders on top" preference, read once: Finder never rereads it mid-session.
fn folders_first() -> bool {
    static FOLDERS_FIRST: OnceLock<bool> = OnceLock::new();
    *FOLDERS_FIRST.get_or_init(|| {
        std::process::Command::new("defaults")
            .args(["read", "com.apple.finder", "_FXSortFoldersFirst"])
            .output()
            .is_ok_and(|out| out.status.success() && out.stdout.trim_ascii() == b"1")
    })
}

/// The window's tick: the watcher's reports, the volume snapshot, the thumbnails that landed.
pub(crate) fn spawn_folder_tick(changes: Arc<Mutex<Vec<PathBuf>>>, cx: &mut Context<FinderDemo>) {
    cx.spawn(async move |this, cx| {
        let mut seen = 0;
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
            if this
                .update(cx, |this, cx| {
                    this.changed(changed, cx);
                    if drawn {
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
    /// Everything the last frame asked for and did not have, plus where the watcher points.
    pub(crate) fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.vanished.take() {
            match self.here() == id {
                true => self.arrive(window, cx),
                false => self.go(id, window, cx),
            }
        }
        for id in self.folders.take_wanted() {
            self.read_folder(id, cx);
        }
        self.sync_stats(cx);
        crate::icons::draw_wanted(&cx.entity(), cx);
        crate::meta::fetch_wanted(&cx.entity(), cx);
        crate::thumbs::frame_drawn();
        self.follow();
    }

    /// What a summary was asked about: a new selection or a fresh listing needs another one.
    pub(crate) fn stats_key(&self) -> (u64, usize) {
        (self.selected.rev(), self.focused_lines().len())
    }

    fn sync_stats(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.stats = None;
            self.stats_pending = None;
            return;
        }
        let key = self.stats_key();
        let known = self.stats.as_ref().is_some_and(|stats| stats.key == key);
        if known || self.stats_pending == Some(key) {
            return;
        }
        self.stats_pending = Some(key);
        let lines = self.focused_lines();
        let chosen = self.selected.snapshot();
        let tags = self.tags.clone();
        cx.spawn(async move |this, cx| {
            let stats = cx
                .background_executor()
                .spawn(async move { preview::summarize(key, &lines, &chosen, &tags) })
                .await;
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

    /// Reads `id` off the main thread: the index's copy first when it has one, then the disk's.
    pub(crate) fn read_folder(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if !id.starts_with('/') || !self.folders.start(id) {
            return;
        }
        if !self.folders.has(id) {
            self.folders.deliver(id, Vec::new(), true);
        }
        let mail = Arc::new(Mutex::new(Mail::default()));
        let filling = mail.clone();
        let sorted_by = (self.sort, self.ascending);
        let art = self.art_size();
        let plain = self.plain();
        let read = cx.background_executor().spawn(async move {
            let dir = Path::new(id);
            let index = index();
            let sort = |rows: &mut Vec<Arc<Row>>| {
                rows.sort_by(|a, b| order(sorted_by.0, sorted_by.1, a, b))
            };
            let mut all: Vec<Arc<Row>> = Vec::new();
            if let Some(known) = index.children(dir).filter(|known| !known.is_empty()) {
                let mut rows = data::rows_of(id, &known, index);
                icons::warm(&rows, art);
                sort(&mut rows);
                let lines = plain.map(|hidden| (hidden, listing::lines_of(&rows, hidden)));
                let mut mail = filling.lock().unwrap();
                mail.batches.push((rows, true));
                mail.lines = lines;
            }
            let post = |mut rows: Vec<Arc<Row>>| {
                icons::warm(&rows, art);
                let mut mail = filling.lock().unwrap();
                let replace = !mail.listed;
                mail.listed = true;
                if replace {
                    sort(&mut rows);
                }
                mail.batches.push((rows, replace));
            };
            let read = finder_fs::list::list_dir_batched(dir, BATCH, |entries| {
                let rows = data::rows_of(id, &entries, index);
                all.extend(rows.iter().cloned());
                post(rows);
            });
            if let Err(error) = read {
                let mut mail = filling.lock().unwrap();
                mail.gone = gone(&error);
                mail.done = true;
                return;
            }
            if finder_fs::icloud::is_drive(dir) {
                let apps = data::icloud_rows(index);
                all.extend(apps.iter().cloned());
                post(apps);
            }
            sort(&mut all);
            let lines = plain.map(|hidden| (hidden, listing::lines_of(&all, hidden)));
            let mut mail = filling.lock().unwrap();
            mail.batches.push((all, true));
            mail.lines = lines;
            mail.done = true;
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let (batches, lines, done, gone) = {
                    let mut mail = mail.lock().unwrap();
                    (
                        std::mem::take(&mut mail.batches),
                        mail.lines.take(),
                        mail.done,
                        mail.gone,
                    )
                };
                let alive = this
                    .update(cx, |this, cx| match gone {
                        true => this.folder_gone(id, cx),
                        false => this.deliver(id, batches, lines, done, sorted_by, cx),
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
            self.folders.finish(id);
            self.landed(id);
        }
        if done && sorted_by != (self.sort, self.ascending) {
            self.sort_folder(id, (self.sort, self.ascending), cx);
        } else if let Some((hidden, lines)) = lines {
            self.built(id, hidden, lines);
        }
        if done {
            self.expand_landed(id);
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

    /// One folder put in the order asked for off the main thread, with the listing built from it
    /// where it is the one on screen: a listing this size cannot be sorted inside a frame.
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
                    rows.sort_by(|a, b| order(by.0, by.1, a, b));
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

    /// A sort that landed: the rows take the order it made and the listing built beside them,
    /// unless a read moved the folder while it ran, which sorts what is there now instead.
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

    /// Finder's re-sort: every listing in memory takes the new order, put in it off the main
    /// thread with the one on screen built there too, whatever shape it is drawn in. The arrow
    /// follows the rows rather than running ahead of them.
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
                        rows.sort_by(|a, b| order(by.0, by.1, a, b));
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

    /// A re-sort that landed: every listing takes the order made for it and the lines built
    /// beside them, unless a read moved one while it ran, which sorts what is there now instead.
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
        // Finder holds the row the cursor is on in view across a re-sort, wherever it moved to.
        if let Some(key) = self.cursor.clone() {
            self.reveal(&key);
        }
    }

    /// Watch what is on screen and nothing else.
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
        // An FSEvents stream reports the whole subtree, so a folder inside a watched one needs
        // no watch of its own.
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
            let id = intern_path(dir);
            if self.folders.has(id) {
                self.read_folder(id, cx);
            }
        }
        cx.background_executor()
            .spawn(async move { index().note_changed(&dirs) })
            .detach();
    }
}
