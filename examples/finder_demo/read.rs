//! Reading folders off the disk: what a frame asked for and did not have, what the watcher says
//! changed, and the order Finder sorts a listing in.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::Context;

use crate::data::{self, Column, Row, intern, intern_path};
use crate::{COLUMN_VIEW, FinderDemo, index, preview};

/// Rows per batch while a folder is read, how often the view takes delivery, and how often the
/// index writes its snapshot.
const BATCH: usize = 512;

const POLL: Duration = Duration::from_millis(5);

const TICK: Duration = Duration::from_millis(120);

/// Ticks between volume snapshots, so the status bar's free space stays roughly current.
const VOLUMES_EVERY: u64 = 40;

/// The rows arriving from one directory read, and whether they replace what is on screen.
#[derive(Default)]
struct Mail {
    batches: Vec<(Vec<Arc<Row>>, bool)>,
    listed: bool,
    done: bool,
}

fn order(sort: Column, ascending: bool, a: &Row, b: &Row) -> Ordering {
    // Finder keeps folders above files regardless of sort column or direction when this
    // preference is on, so it is applied before the ascending/descending reversal below.
    // The Kind column groups folders first unconditionally, preference or not: a folder
    // doesn't really have a "kind" comparable to a file's, so Finder never interleaves them
    // alphabetically there (verified against real Finder: "Folder" sorts ahead of "Document"
    // even though that is backwards alphabetically).
    if folders_first() || sort == Column::Kind {
        let by_kind = b.opens.is_some().cmp(&a.opens.is_some());
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

/// Whether Finder's "keep folders on top" preference is on. Finder itself never rereads this
/// mid-session, so this demo doesn't either: `defaults` is shelled out to once and cached.
fn folders_first() -> bool {
    static FOLDERS_FIRST: OnceLock<bool> = OnceLock::new();
    *FOLDERS_FIRST.get_or_init(|| {
        std::process::Command::new("defaults")
            .args(["read", "com.apple.finder", "_FXSortFoldersFirst"])
            .output()
            .is_ok_and(|out| out.status.success() && out.stdout.trim_ascii() == b"1")
    })
}

/// The window's own tick: the watcher's reports, the volume snapshot and the thumbnails that
/// landed since the last one.
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

/// The queue reports from its own thread; the window takes delivery on the same tick.
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
    /// Start a read for every folder the last frame asked for and did not have, and keep the
    /// watcher pointed at what is on screen.
    pub(crate) fn pump(&mut self, cx: &mut Context<Self>) {
        for id in self.folders.take_wanted() {
            self.read_folder(id, cx);
        }
        self.sync_stats(cx);
        if self.column_settling.replace(false) {
            cx.notify();
        }
        crate::thumbs::frame_drawn();
        self.follow();
    }

    /// What the last selection summary was asked about: a new selection or a fresh listing needs
    /// another one.
    pub(crate) fn stats_key(&self) -> (u64, usize) {
        (self.selected.rev(), self.focused_lines().len())
    }

    /// Counting and weighing the selection walks every chosen row, which the card and the status
    /// bar cannot do on a frame: one pass runs off the main thread per change and they say
    /// "Calculating…" until it lands.
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

    /// Reads `id` off the main thread, handing rows over as they arrive: the index's copy first
    /// when it has one, then the disk's, which replaces it.
    pub(crate) fn read_folder(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if !id.starts_with('/') || !self.folders.start(id) {
            return;
        }
        if !self.folders.has(id) {
            self.folders.deliver(id, Vec::new(), true);
        }
        let mail = Arc::new(Mutex::new(Mail::default()));
        let filling = mail.clone();
        let read = cx.background_executor().spawn(async move {
            let dir = Path::new(id);
            let index = index();
            if let Some(known) = index.children(dir).filter(|known| !known.is_empty()) {
                let rows = data::rows_of(id, &known, index);
                filling.lock().unwrap().batches.push((rows, true));
            }
            let post = |rows: Vec<Arc<Row>>| {
                let mut mail = filling.lock().unwrap();
                let replace = !mail.listed;
                mail.listed = true;
                mail.batches.push((rows, replace));
            };
            let read = finder_fs::list::list_dir_batched(dir, BATCH, |entries| {
                post(data::rows_of(id, &entries, index))
            });
            let mut mail = filling.lock().unwrap();
            if read.is_ok() && !mail.listed {
                mail.batches.push((Vec::new(), true));
            }
            mail.done = true;
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let (batches, done) = {
                    let mut mail = mail.lock().unwrap();
                    (std::mem::take(&mut mail.batches), mail.done)
                };
                let alive = this
                    .update(cx, |this, cx| this.deliver(id, batches, done, cx))
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
        done: bool,
        cx: &mut Context<Self>,
    ) {
        let landed = !batches.is_empty();
        for (rows, replace) in batches {
            self.folders.deliver(id, rows, replace);
        }
        if done {
            self.folders.finish(id);
        }
        // A re-read sorts once, when the whole listing has landed and taken the old one's place.
        if (landed || done) && !self.folders.staging(id) {
            self.sort_folder(id);
        }
        if let Some(key) = self.reveal_next.clone() {
            if self
                .folders
                .get(id)
                .is_some_and(|folder| folder.rows.iter().any(|row| row.path == key))
            {
                self.reveal_next = None;
                self.reveal(&key);
            }
        }
        if landed || done {
            cx.notify();
        }
    }

    fn sort_folder(&mut self, id: &'static str) {
        let (sort, ascending) = (self.sort, self.ascending);
        if let Some(folder) = self.folders.get_mut(id) {
            folder.rows.sort_by(|a, b| order(sort, ascending, a, b));
        }
    }

    pub(crate) fn resort(&mut self) {
        for id in self.folders.cached() {
            self.sort_folder(id);
        }
    }

    /// Watch what is on screen and nothing else: the folder, the disclosed subfolders, the
    /// browser's chain.
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
        for id in &wanted {
            if self.watching.insert(id) {
                self.watcher.watch(Path::new(id));
            }
        }
        let watcher = &self.watcher;
        self.watching.retain(|id| {
            let keep = wanted.contains(id);
            if !keep {
                watcher.unwatch(Path::new(id));
            }
            keep
        });
    }

    /// The watcher's report: re-read what changed and is on screen, and tell the index.
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
