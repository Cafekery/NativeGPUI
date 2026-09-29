use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use finder_fs::entry::{Entry, EntryKind};
use finder_fs::list::Identity;
use gpui::{App, BackgroundExecutor, Context, Task, Window};

use crate::app::FinderDemo;
use crate::data::{self, Column, Facts, Line, Row, intern, intern_path};
use crate::listing::{Shape, Walk};
use crate::signal::Signal;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, LIST_VIEW};
use crate::{icons, index, listing, preview};

const BATCH: usize = 64;
const TICK: Duration = Duration::from_millis(120);
const STATS_NOW: usize = 256;
const REREAD: usize = 256;
/// Below this, the disk lists a folder before fseventsd can vouch for the index.
const BIG: usize = 20_000;

enum Trouble {
    Renamed(&'static str),
    Gone(&'static str),
    Refused(io::Error),
}

type Sorted = (Vec<Arc<Row>>, Option<(bool, Vec<Line>)>);

type Resorted = (HashMap<&'static str, Vec<Arc<Row>>>, Shape, Walk);

struct Listed {
    first: Option<(Vec<Arc<Row>>, (bool, Vec<Line>))>,
    rest: Task<Result<Sorted, Trouble>>,
}

struct Key {
    discloses: bool,
    missing: bool,
    at: f64,
    rank: u64,
}

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

fn by(keys: &[Key], (sort, ascending): (Column, bool)) -> impl Fn(usize, usize) -> Ordering + '_ {
    let first = crate::prefs::folders_first() && sort == Column::Name || sort == Column::Kind;
    move |a, b| {
        let (a, b) = (&keys[a], &keys[b]);
        let column = match sort {
            Column::Name => Ordering::Equal,
            sort if sort.dated() => a.at.total_cmp(&b.at),
            _ => a.rank.cmp(&b.rank),
        };
        let folders = match first {
            true => b.discloses.cmp(&a.discloses),
            false => Ordering::Equal,
        };
        // Finder lists what a column says nothing for last, whichever way it sorts.
        folders.then(a.missing.cmp(&b.missing)).then(if ascending {
            column
        } else {
            column.reverse()
        })
    }
}

fn text_ranks<'a>(texts: impl Iterator<Item = Option<&'a str>> + Clone) -> Vec<u64> {
    let distinct: Vec<&str> = texts
        .clone()
        .flatten()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let ranks = finder_fs::sort::ranks(distinct.iter().copied());
    let rank: HashMap<&str, u32> = distinct.into_iter().zip(ranks).collect();
    texts
        .map(|text| text.map_or(0, |text| u64::from(rank[text])))
        .collect()
}

fn entry_date(entry: &Entry, sort: Column) -> f64 {
    match sort {
        Column::Modified => data::modified_at(entry),
        Column::Created => data::reference_seconds(entry.created),
        Column::Added => entry
            .added
            .map_or(f64::NEG_INFINITY, data::reference_seconds),
        _ => 0.,
    }
}

fn names_descend((sort, ascending): (Column, bool)) -> bool {
    sort == Column::Name && !ascending
}

pub(crate) fn sort_rows(rows: &mut Vec<Arc<Row>>, sorted_by: (Column, bool), extras: bool) {
    let sort = sorted_by.0;
    if extras || sort.extra() {
        data::read_extras(rows);
    }
    let ranks = match sort {
        Column::Size => rows.iter().map(|row| row.bytes).collect(),
        sort if sort == Column::Name || sort.dated() => vec![0; rows.len()],
        sort => text_ranks(rows.iter().map(|row| row.words(sort))),
    };
    let keys: Vec<Key> = rows
        .iter()
        .zip(ranks)
        .map(|(row, rank)| {
            let at = row.date(sort);
            Key {
                discloses: row.discloses,
                missing: sort.extra() && at.is_none() && row.words(sort).is_none()
                    || sort == Column::Added && at.is_none(),
                at: at.unwrap_or(f64::NEG_INFINITY),
                rank,
            }
        })
        .collect();
    let order = finder_fs::sort::order(
        rows.len(),
        by(&keys, sorted_by),
        |at| &rows[at].name,
        names_descend(sorted_by),
    );
    let mut held: Vec<Option<Arc<Row>>> = rows.drain(..).map(Some).collect();
    rows.extend(order.into_iter().filter_map(|at| held[at].take()));
}

/// With `coarse` whole-second dates, every entry that could be among the first `count` instead.
fn first_screen(
    id: &'static str,
    entries: &[&Entry],
    folders: &HashMap<String, Facts>,
    sorted_by: (Column, bool),
    hidden: bool,
    count: usize,
    coarse: bool,
) -> Option<Vec<usize>> {
    let sort = sorted_by.0;
    let listed: Vec<usize> = (0..entries.len())
        .filter(|&at| hidden || !entries[at].hidden)
        .collect();
    let ranks = match sort {
        Column::Kind => {
            let kinds: Vec<_> = listed
                .iter()
                .map(|&at| data::kind_at(&data::join(id, &entries[at].name), entries[at]).label)
                .collect();
            text_ranks(kinds.iter().map(|kind| Some(&**kind)))
        }
        Column::Size => listed
            .iter()
            .map(|&at| {
                let entry = entries[at];
                match data::bytes_of(entry, data::facts_of(entry, folders).0) {
                    None if entry.package => None,
                    bytes => Some(bytes.unwrap_or(0)),
                }
            })
            .collect::<Option<_>>()?,
        sort if sort.extra() => return None,
        _ => vec![0; listed.len()],
    };
    let mut keys: Vec<Key> = listed
        .iter()
        .zip(ranks)
        .map(|(&at, rank)| Key {
            discloses: entries[at].is_dir(),
            missing: sort == Column::Added && entries[at].added.is_none(),
            at: entry_date(entries[at], sort),
            rank,
        })
        .collect();
    let first = finder_fs::sort::first(
        listed.len(),
        count,
        by(&keys, sorted_by),
        |at| &entries[listed[at]].name,
        names_descend(sorted_by),
    );
    let Some(&last) = first.last().filter(|_| coarse) else {
        return Some(first.into_iter().map(|at| listed[at]).collect());
    };
    let (edge, later) = (keys.len(), if sorted_by.1 { 1. } else { -1. });
    keys.push(Key {
        at: keys[last].at + later,
        ..keys[last]
    });
    let before = by(&keys, sorted_by);
    let near = (0..listed.len()).filter(|&at| before(at, edge) != Ordering::Greater);
    Some(near.map(|at| listed[at]).collect())
}

type Batches = Vec<(Arc<Vec<Entry>>, Task<Vec<Arc<Row>>>)>;

fn read_batches(
    id: &'static str,
    folders: &Arc<HashMap<String, Facts>>,
    extras: bool,
    executor: &BackgroundExecutor,
) -> Result<Batches, Trouble> {
    let mut batches: Batches = Vec::new();
    finder_fs::list::list_dir_batched(Path::new(id), BATCH, |entries| {
        batches.push(batch(id, entries, folders.clone(), extras, executor));
    })
    .map_err(|error| trouble(id, error))?;
    if let Some(system) = system_twin(id) {
        let listed: HashSet<&str> = batches
            .iter()
            .flat_map(|(entries, _)| entries.iter().map(|entry| entry.name.as_str()))
            .collect();
        let apps: Vec<Entry> = finder_fs::list::list_dir(Path::new(system))
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| !listed.contains(entry.name.as_str()))
            .collect();
        let facts = Arc::new(index().folders_in(Path::new(system)));
        batches.push(batch(system, apps, facts, extras, executor));
    }
    remember(id);
    Ok(batches)
}

fn batch(
    dir: &'static str,
    entries: Vec<Entry>,
    facts: Arc<HashMap<String, Facts>>,
    extras: bool,
    executor: &BackgroundExecutor,
) -> (Arc<Vec<Entry>>, Task<Vec<Arc<Row>>>) {
    ask_for_apps(dir, &entries);
    let entries = Arc::new(entries);
    let listed = entries.clone();
    let rows = executor.spawn(async move { data::rows_of(dir, listed.iter(), &facts, extras) });
    (entries, rows)
}

fn system_twin(id: &str) -> Option<&'static str> {
    match id {
        "/Applications" => Some("/System/Applications"),
        "/Applications/Utilities" => Some("/System/Applications/Utilities"),
        _ => None,
    }
}

fn ask_for_apps<'a>(id: &str, entries: impl IntoIterator<Item = &'a Entry>) {
    let apps = entries
        .into_iter()
        .filter(|entry| entry.package && entry.name.ends_with(".app"));
    icons::ask_app_management(apps.map(|entry| PathBuf::from(data::join(id, &entry.name))));
}

async fn finish(
    id: &'static str,
    batches: Batches,
    shared: Option<(Vec<usize>, Vec<Arc<Row>>)>,
    (sorted_by, plain, extras): ((Column, bool), Option<bool>, bool),
    (count, art): (usize, f32),
) -> Sorted {
    let mut all: Vec<Arc<Row>> = Vec::new();
    for (_, rows) in batches {
        all.extend(rows.await);
    }
    for (at, row) in shared
        .into_iter()
        .flat_map(|(chosen, rows)| chosen.into_iter().zip(rows))
    {
        all[at] = row;
    }
    if finder_fs::icloud::is_drive(Path::new(id)) {
        all.extend(data::icloud_rows(index()));
    }
    data::weigh_packages(&mut all);
    sort_rows(&mut all, sorted_by, extras);
    icons::prepare(&all, count, art);
    let lines = plain.map(|hidden| (hidden, listing::lines_of(&all, hidden)));
    (all, lines)
}

fn screen_rows<'a>(
    id: &'static str,
    entries: impl Iterator<Item = &'a Entry> + Clone,
    folders: &HashMap<String, Facts>,
    extras: bool,
    (count, art): (usize, f32),
) -> Vec<Arc<Row>> {
    ask_for_apps(id, entries.clone());
    let early = (entries.clone())
        .filter_map(|entry| data::early_artwork(&data::join(id, &entry.name), entry));
    let drawing = icons::begin(early, art);
    let rows = data::rows_of(id, entries, folders, extras);
    drawing.finish();
    icons::prepare(&rows, count, art);
    rows
}

fn indexed_screen(
    id: &'static str,
    folders: &HashMap<String, Facts>,
    sorted_by: (Column, bool),
    hidden: bool,
    count: usize,
) -> Option<Vec<Entry>> {
    let dir = Path::new(id);
    let index = index();
    let seconds =
        |at: std::time::SystemTime| Some(at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs());
    let named = sorted_by.0 == Column::Name;
    let known = match named {
        true => {
            let now = std::fs::metadata(dir).ok()?.modified().ok()?;
            (seconds(index.modified(dir)?)? == seconds(now)?).then_some(())?;
            index.children(dir)?
        }
        false => {
            (index.visible_children(dir)? >= BIG).then_some(())?;
            index.current_children(dir)?
        }
    };
    let coarse = sorted_by.0 == Column::Modified;
    let listed: Vec<&Entry> = known.iter().collect();
    let chosen = first_screen(id, &listed, folders, sorted_by, hidden, count, coarse)?;
    (!chosen.is_empty() && chosen.len() <= REREAD).then_some(())?;
    let read: Vec<Entry> = chosen
        .into_iter()
        .map(|at| {
            let was = &known[at];
            let entry = finder_fs::list::listed_entry_at(&dir.join(&was.name)).ok()?;
            let same = (&entry.name, entry.kind, entry.package, entry.hidden)
                == (&was.name, was.kind, was.package, was.hidden)
                && (named
                    || seconds(entry.modified) == seconds(was.modified)
                        && (entry.kind == EntryKind::Directory || entry.size == was.size));
            same.then_some(entry)
        })
        .collect::<Option<_>>()?;
    if !coarse {
        return Some(read);
    }
    let refs: Vec<&Entry> = read.iter().collect();
    let exact = first_screen(id, &refs, folders, sorted_by, hidden, count, false)?;
    Some(exact.into_iter().map(|at| read[at].clone()).collect())
}

fn list(
    id: &'static str,
    sorted_by: (Column, bool),
    (first, plain): (Option<bool>, Option<bool>),
    (screen, extras): ((usize, f32), bool),
    executor: BackgroundExecutor,
) -> Result<Listed, Trouble> {
    let count = screen.0;
    let folders = Arc::new(index().folders_in(Path::new(id)));
    let icloud = finder_fs::icloud::is_drive(Path::new(id));
    let twin = system_twin(id);
    let read = {
        let (folders, executor) = (folders.clone(), executor.clone());
        move || read_batches(id, &folders, extras, &executor)
    };
    // A date or size sort's pick from the index waits on fseventsd, so its disk pass starts first.
    let early = (sorted_by.0 != Column::Name).then(|| std::thread::spawn(read.clone()));
    let batches = move || early.map_or_else(read, |early| early.join().unwrap());
    let shown = |rows: Vec<Arc<Row>>, hidden: bool| {
        let lines = listing::lines_of(&rows, hidden);
        (rows, (hidden, lines))
    };
    let vouched = matches!(sorted_by.0, Column::Name | Column::Modified | Column::Size);
    let known = first
        .filter(|_| vouched && !icloud && twin.is_none())
        .and_then(|hidden| {
            let entries = indexed_screen(id, &folders, sorted_by, hidden, count)?;
            let rows = screen_rows(id, entries.iter(), &folders, extras, screen);
            Some(shown(rows, hidden))
        });
    if known.is_some() {
        let rest = executor.spawn(async move {
            Ok(finish(id, batches()?, None, (sorted_by, plain, extras), screen).await)
        });
        return Ok(Listed { first: known, rest });
    }
    let batches = batches()?;
    let entries: Vec<&Entry> = batches
        .iter()
        .flat_map(|(entries, _)| entries.iter())
        .collect();
    let own = entries.len() - twin.map_or(0, |_| batches.last().map_or(0, |(twin, _)| twin.len()));
    let (shared, first) = first
        .filter(|_| !icloud)
        .and_then(|hidden| {
            let chosen = first_screen(id, &entries, &folders, sorted_by, hidden, count, false)?;
            let rows_in = |dir, facts: &HashMap<String, Facts>, ours: bool| {
                let picked = chosen.iter().filter(move |&&at| (at < own) == ours);
                screen_rows(dir, picked.map(|&at| entries[at]), facts, extras, screen)
            };
            let rows = match twin {
                None => rows_in(id, &folders, true),
                Some(twin) => {
                    let facts = index().folders_in(Path::new(twin));
                    let mut ours = rows_in(id, &folders, true).into_iter();
                    let mut theirs = rows_in(twin, &facts, false).into_iter();
                    chosen
                        .iter()
                        .filter_map(|&at| match at < own {
                            true => ours.next(),
                            false => theirs.next(),
                        })
                        .collect()
                }
            };
            Some(((chosen, rows.clone()), (rows, hidden)))
        })
        .unzip();
    let rest = executor.spawn(async move {
        Ok(finish(id, batches, shared, (sorted_by, plain, extras), screen).await)
    });
    let first = first.map(|(rows, hidden)| shown(rows, hidden));
    Ok(Listed { first, rest })
}

pub(crate) static EVENTS: Signal = Signal::new();

pub(crate) fn follow_events(changes: Arc<Mutex<Vec<PathBuf>>>, cx: &mut Context<FinderDemo>) {
    cx.spawn(async move |this, cx| {
        let (mut seen, mut seen_tags) = (0, 0);
        loop {
            seen = EVENTS.after(seen).await;
            let changed = std::mem::take(&mut *changes.lock().unwrap());
            let tags = crate::tags::tags_landed();
            let tagged = std::mem::replace(&mut seen_tags, tags) != tags;
            if this
                .update(cx, |this, cx| {
                    this.changed(changed, cx);
                    let events = std::mem::take(&mut *this.queue_events.lock().unwrap());
                    this.handle_queue_events(events, cx);
                    if tagged {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
            cx.background_executor().timer(TICK).await;
        }
    })
    .detach();
}

pub(crate) fn notify_windows(cx: &mut App) {
    for window in cx.windows() {
        if let Some(demo) = window.downcast::<FinderDemo>() {
            demo.update(cx, |_, _, cx| cx.notify()).ok();
        }
    }
}

impl FinderDemo {
    pub(crate) fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settle_vanished(window, cx);
        for id in self.folders.take_wanted() {
            self.read_folder(id, cx);
        }
        crate::icons::draw_wanted(&cx.entity(), cx);
        crate::meta::fetch_wanted(&cx.entity(), cx);
        crate::custom::fetch_wanted(&cx.entity(), cx);
        crate::thumbs::frame_drawn();
        // A row asks for its thumbnail as the list lays itself out, which is after this runs.
        cx.defer(|_| crate::thumbs::collect_thumbnails());
        self.follow();
    }

    pub(crate) fn stats_key(&self) -> (u64, usize) {
        (self.selected.rev(), self.focused_lines().len())
    }

    pub(crate) fn sync_stats(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.stats = None;
            self.stats_pending = None;
            self.stats_stale.store(true, Relaxed);
            return;
        }
        let key = self.stats_key();
        let known = self.stats.as_ref().is_some_and(|stats| stats.key == key);
        if known || self.stats_pending == Some(key) {
            return;
        }
        self.stats_stale.store(true, Relaxed);
        self.stats_stale = Arc::default();
        let lines = self.focused_lines();
        if self.selected.len() <= STATS_NOW {
            let chosen = self.chosen_in(&lines, usize::MAX);
            if chosen
                .iter()
                .all(|line| preview::sized_by_index(&line.row).is_none())
            {
                self.stats_pending = None;
                self.stats = preview::summarize(key, &chosen, &self.tags, &self.stats_stale);
                return;
            }
        }
        self.stats_pending = Some(key);
        let stale = self.stats_stale.clone();
        let chosen = self.selected.snapshot();
        let tags = self.tags.clone();
        cx.spawn(async move |this, cx| {
            let summary = cx
                .background_executor()
                .spawn(async move {
                    let picked = lines.iter().filter(|line| chosen.contains(&line.key));
                    preview::summarize(key, picked, &tags, &stale)
                })
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
        if crate::locations::is_location(id) {
            self.read_location(id, cx);
            return;
        }
        if !id.starts_with('/') || !self.folders.start(id) {
            return;
        }
        let fresh = !self.folders.has(id);
        if fresh {
            self.folders.deliver(id, Vec::new(), true);
        }
        let empty = self
            .folders
            .get(id)
            .is_some_and(|folder| folder.rows.is_empty());
        let sorted_by = (self.sort, self.ascending);
        let headed = self.headed_for() == Some(id);
        let (plain, extras) = match headed {
            true => (self.plain_on_arrival(), self.extras_in(id)),
            false => (self.plain(), self.wants_extras()),
        };
        // A disclosed folder shows its rows at once: rows landing later would push down those below.
        let first = plain.filter(|_| empty && (headed || id == self.here()));
        let screen = ((self.on_screen(), self.art_size()), extras);
        let executor = cx.background_executor().clone();
        let listed = cx
            .background_executor()
            .spawn(async move { list(id, sorted_by, (first, plain), screen, executor) });
        cx.spawn(async move |this, cx| {
            let rest = match listed.await {
                Ok(Listed { first, rest }) => {
                    if let Some((rows, lines)) = first {
                        let alive = this.update(cx, |this, cx| {
                            this.deliver(id, Some(rows), Some(lines), false, sorted_by, cx)
                        });
                        if alive.is_err() {
                            return;
                        }
                    }
                    rest.await
                }
                Err(trouble) => Err(trouble),
            };
            this.update(cx, |this, cx| match rest {
                Ok((rows, lines)) => this.deliver(id, Some(rows), lines, true, sorted_by, cx),
                Err(trouble) => this.troubled(id, trouble, fresh, sorted_by, cx),
            })
            .ok();
        })
        .detach();
    }

    fn troubled(
        &mut self,
        id: &'static str,
        trouble: Trouble,
        fresh: bool,
        sorted_by: (Column, bool),
        cx: &mut Context<Self>,
    ) {
        match trouble {
            Trouble::Renamed(now) => self.folder_renamed(id, now, cx),
            Trouble::Gone(above) => self.folder_gone(id, above, cx),
            Trouble::Refused(error) => {
                self.deliver(id, None, None, true, sorted_by, cx);
                if fresh {
                    self.folder_refused(id, &error, cx);
                }
            }
        }
    }

    pub(crate) fn deliver(
        &mut self,
        id: &'static str,
        rows: Option<Vec<Arc<Row>>>,
        lines: Option<(bool, Vec<Line>)>,
        done: bool,
        sorted_by: (Column, bool),
        cx: &mut Context<Self>,
    ) {
        let arrived = rows.is_some() && self.arrived(id, cx);
        let mut landed = lines.is_some();
        if let Some(rows) = rows {
            landed |= self.folders.deliver(id, rows, true);
        }
        if done && self.folders.finish(id) {
            self.read_folder(id, cx);
        }
        if done && sorted_by != (self.sort, self.ascending) {
            self.sort_folder(id, (self.sort, self.ascending), cx);
        } else if let Some((hidden, lines)) = lines {
            self.built(id, hidden, lines);
        }
        if done {
            self.expand_landed(id);
        }
        if arrived || (done && id == self.here() && self.view == GALLERY_VIEW) {
            self.settle();
        }
        let folders = &self.folders;
        let landed_row = self.reveal_next.take_if(|key| {
            folders
                .get(id)
                .is_some_and(|folder| folder.rows.iter().any(|row| row.path == *key))
        });
        if let Some(key) = landed_row {
            self.reveal(&key);
        }
        if landed || done {
            cx.notify();
        }
    }

    fn sort_folder(&mut self, id: &'static str, by: (Column, bool), cx: &mut Context<Self>) {
        let Some(mut rows) = self.folders.get(id).map(|folder| folder.rows.to_vec()) else {
            return;
        };
        let was = rows.len();
        let plain = self.plain().filter(|_| id == self.here());
        let extras = self.wants_extras();
        cx.spawn(async move |this, cx| {
            let sorted = cx
                .background_executor()
                .spawn(async move {
                    sort_rows(&mut rows, by, extras);
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
        (rows, lines): Sorted,
        cx: &mut Context<Self>,
    ) {
        let now = (self.sort, self.ascending);
        if now != by
            || self
                .folders
                .get(id)
                .is_none_or(|folder| folder.rows.len() != was)
        {
            self.sort_folder(id, now, cx);
            return;
        }
        if let Some(folder) = self.folders.get_mut(id) {
            folder.rows = rows.into();
        }
        if let Some((hidden, lines)) = lines {
            self.built(id, hidden, lines);
        }
    }

    pub(crate) fn sort_held(&mut self, by: (Column, bool), cx: &mut Context<Self>) {
        let held = self.folders.held();
        let was: Vec<(&'static str, usize)> =
            held.iter().map(|(id, rows)| (*id, rows.len())).collect();
        let view = self.pending_view.unwrap_or(self.view);
        let shape = self.shape_sorted(view == LIST_VIEW, by);
        let extras = self.wants_extras();
        cx.spawn(async move |this, cx| {
            let sorted = cx
                .background_executor()
                .spawn(async move {
                    let mut folders: HashMap<&'static str, Vec<Arc<Row>>> =
                        held.into_iter().collect();
                    for rows in folders.values_mut() {
                        sort_rows(rows, by, extras);
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
        (mut folders, shape, walk): Resorted,
        cx: &mut Context<Self>,
    ) {
        let wanted = self.view_sort(self.pending_view.unwrap_or(self.view));
        if by != wanted {
            return self.sort_held(wanted, cx);
        }
        let turned = by != (self.sort, self.ascending);
        (self.sort, self.ascending) = by;
        let mut moved = false;
        for (id, count) in was {
            let Some(rows) = folders.remove(id) else {
                continue;
            };
            match self.folders.get_mut(id) {
                Some(folder) if folder.rows.len() == *count => folder.rows = rows.into(),
                Some(_) => {
                    moved = true;
                    self.sort_folder(id, by, cx);
                }
                None => moved = true,
            }
        }
        if let Some(view) = self.pending_view.take() {
            self.show_view(view);
        }
        if !moved {
            self.shaped(&shape, walk);
        }
        if let Some(key) = self.cursor.clone().filter(|_| turned) {
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
        let outermost: HashSet<&'static str> = wanted
            .iter()
            .copied()
            .filter(|id| id.starts_with('/') && !inside(id))
            .collect();
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
                match self.folders.shown(id) {
                    true => self.read_folder(id, cx),
                    false => self.folders.stale(id),
                }
            }
        }
        cx.background_executor()
            .spawn(async move { index().note_changed(&dirs) })
            .detach();
    }
}
