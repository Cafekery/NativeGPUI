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
use crate::data::{self, Bands, Column, Face, Facts, Line, Order, Row, intern, intern_path};
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
    band: Option<f64>,
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

fn by(keys: &[Key], order: Order) -> impl Fn(usize, usize) -> Ordering + '_ {
    let Order {
        group,
        sort,
        ascending,
    } = order;
    let first = crate::prefs::folders_first() && sort == Column::Name || sort == Column::Kind;
    let up = group.is_none_or(Column::first_ascending);
    move |a, b| {
        let (a, b) = (&keys[a], &keys[b]);
        let band = match (a.band, b.band) {
            (Some(a), Some(b)) if up => a.total_cmp(&b),
            (Some(a), Some(b)) => b.total_cmp(&a),
            (a, b) => b.is_some().cmp(&a.is_some()),
        };
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
        band.then(folders)
            .then(a.missing.cmp(&b.missing))
            .then(if ascending { column } else { column.reverse() })
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

fn text_places<'a>(texts: impl Iterator<Item = Option<&'a str>> + Clone) -> Vec<Option<f64>> {
    let ranks = text_ranks(texts.clone());
    texts
        .zip(ranks)
        .map(|(text, rank)| text.map(|_| rank as f64))
        .collect()
}

/// Folders' band first, as a Kind sort puts them.
fn kind_places<'a>(kinds: impl Iterator<Item = (bool, &'a str)> + Clone) -> Vec<Option<f64>> {
    let ranks = text_ranks(kinds.clone().map(|(_, kind)| Some(kind)));
    kinds
        .zip(ranks)
        .map(|((folder, _), rank)| Some(rank as f64 - if folder { 2f64.powi(32) } else { 0. }))
        .collect()
}

/// Where each row's band sorts; a row in no band goes last whichever way.
fn band_places(rows: &[Arc<Row>], group: Option<Column>) -> Vec<Option<f64>> {
    let Some(group) = group else {
        return vec![None; rows.len()];
    };
    match group {
        Column::Name => {
            let initials: Vec<_> = rows.iter().map(|row| Bands::initial(&row.name)).collect();
            text_places(initials.iter().map(|initial| Some(&**initial)))
        }
        Column::Kind => kind_places(rows.iter().map(|row| (row.discloses, &*row.kind))),
        Column::Size => rows
            .iter()
            .map(|row| Some(Bands::size_place(row.face == Face::Folder, row.bytes)))
            .collect(),
        group if group.dated() => {
            let mut bands = Bands::new(group);
            rows.iter()
                .map(|row| row.date(group).map(|at| bands.place(at)))
                .collect()
        }
        group => text_places(
            rows.iter()
                .map(|row| row.words(group).filter(|words| !words.is_empty())),
        ),
    }
}

fn entry_date(entry: &Entry, column: Column) -> Option<f64> {
    match column {
        Column::Modified => Some(data::modified_at(entry)),
        Column::Created => Some(data::reference_seconds(entry.created)),
        Column::Added => entry.added.map(data::reference_seconds),
        _ => None,
    }
}

fn names_descend(order: Order) -> bool {
    order.sort == Column::Name && !order.ascending
}

pub(crate) fn sort_rows(rows: &mut Vec<Arc<Row>>, order: Order, extras: bool) {
    let sort = order.sort;
    if extras || sort.extra() || order.group.is_some_and(Column::extra) {
        data::read_extras(rows);
    }
    if order.group == Some(Column::Application) {
        data::read_apps(rows);
    }
    let ranks = match sort {
        Column::Size => rows.iter().map(|row| row.bytes).collect(),
        sort if sort == Column::Name || sort.dated() => vec![0; rows.len()],
        sort => text_ranks(rows.iter().map(|row| row.words(sort))),
    };
    let bands = band_places(rows, order.group);
    let keys: Vec<Key> = rows
        .iter()
        .zip(ranks)
        .zip(bands)
        .map(|((row, rank), band)| {
            let at = row.date(sort);
            Key {
                discloses: row.discloses,
                missing: sort.extra() && at.is_none() && row.words(sort).is_none()
                    || sort == Column::Added && at.is_none(),
                at: at.unwrap_or(f64::NEG_INFINITY),
                rank,
                band,
            }
        })
        .collect();
    let placed = finder_fs::sort::order(
        rows.len(),
        by(&keys, order),
        |at| &rows[at].name,
        names_descend(order),
    );
    let mut held: Vec<Option<Arc<Row>>> = rows.drain(..).map(Some).collect();
    rows.extend(placed.into_iter().filter_map(|at| held[at].take()));
}

/// With `coarse` whole-second dates, every entry that could be among the first `count` instead.
fn first_screen(
    id: &'static str,
    entries: &[&Entry],
    folders: &HashMap<String, Facts>,
    order: Order,
    hidden: bool,
    count: usize,
    coarse: bool,
) -> Option<Vec<usize>> {
    let (sort, group) = (order.sort, order.group);
    let listed: Vec<usize> = (0..entries.len())
        .filter(|&at| hidden || !entries[at].hidden)
        .collect();
    let kinds: Vec<_> = match sort == Column::Kind || group == Some(Column::Kind) {
        true => listed
            .iter()
            .map(|&at| data::kind_at(&data::join(id, &entries[at].name), entries[at]).label)
            .collect(),
        false => Vec::new(),
    };
    let bytes: Vec<u64> = match sort == Column::Size || group == Some(Column::Size) {
        true => listed
            .iter()
            .map(|&at| {
                let entry = entries[at];
                match data::bytes_of(entry, data::facts_of(entry, folders).0) {
                    None if entry.package => None,
                    bytes => Some(bytes.unwrap_or(0)),
                }
            })
            .collect::<Option<_>>()?,
        false => Vec::new(),
    };
    let ranks = match sort {
        Column::Kind => text_ranks(kinds.iter().map(|kind| Some(&**kind))),
        Column::Size => bytes.clone(),
        sort if sort.extra() => return None,
        _ => vec![0; listed.len()],
    };
    let bands: Vec<Option<f64>> = match group {
        None => vec![None; listed.len()],
        Some(Column::Name) => {
            let initials: Vec<_> = (listed.iter())
                .map(|&at| Bands::initial(&entries[at].name))
                .collect();
            text_places(initials.iter().map(|initial| Some(&**initial)))
        }
        Some(Column::Kind) => kind_places(
            (listed.iter().zip(&kinds)).map(|(&at, kind)| (entries[at].is_dir(), &**kind)),
        ),
        Some(Column::Size) => (listed.iter().zip(&bytes))
            .map(|(&at, &bytes)| Some(Bands::size_place(entries[at].is_dir(), bytes)))
            .collect(),
        Some(group) if group.dated() && !group.extra() => {
            let mut bands = Bands::new(group);
            (listed.iter())
                .map(|&at| entry_date(entries[at], group).map(|day| bands.place(day)))
                .collect()
        }
        Some(_) => return None,
    };
    let mut keys: Vec<Key> = listed
        .iter()
        .zip(ranks)
        .zip(bands)
        .map(|((&at, rank), band)| Key {
            discloses: entries[at].is_dir(),
            missing: sort == Column::Added && entries[at].added.is_none(),
            at: entry_date(entries[at], sort).unwrap_or(f64::NEG_INFINITY),
            rank,
            band,
        })
        .collect();
    let first = finder_fs::sort::first(
        listed.len(),
        count,
        by(&keys, order),
        |at| &entries[listed[at]].name,
        names_descend(order),
    );
    let Some(&last) = first.last().filter(|_| coarse) else {
        return Some(first.into_iter().map(|at| listed[at]).collect());
    };
    let (edge, later) = (keys.len(), if order.ascending { 1. } else { -1. });
    keys.push(Key {
        at: keys[last].at + later,
        ..keys[last]
    });
    let before = by(&keys, order);
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
        ask_for_apps(id, &entries);
        let (entries, facts) = (Arc::new(entries), folders.clone());
        let listed = entries.clone();
        let rows = executor.spawn(async move { data::rows_of(id, listed.iter(), &facts, extras) });
        batches.push((entries, rows));
    })
    .map_err(|error| trouble(id, error))?;
    remember(id);
    Ok(batches)
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
    (order, plain, extras): (Order, Option<bool>, bool),
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
    sort_rows(&mut all, order, extras);
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
    order: Order,
    hidden: bool,
    count: usize,
) -> Option<Vec<Entry>> {
    let dir = Path::new(id);
    let index = index();
    let seconds =
        |at: std::time::SystemTime| Some(at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs());
    let named = by_name(order);
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
    let coarse = order.sort == Column::Modified;
    let listed: Vec<&Entry> = known.iter().collect();
    let chosen = first_screen(id, &listed, folders, order, hidden, count, coarse)?;
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
    let exact = first_screen(id, &refs, folders, order, hidden, count, false)?;
    Some(exact.into_iter().map(|at| read[at].clone()).collect())
}

fn by_name(order: Order) -> bool {
    order.sort == Column::Name && order.group.is_none_or(|group| group == Column::Name)
}

fn list(
    id: &'static str,
    order: Order,
    (first, plain): (Option<bool>, Option<bool>),
    (screen, extras): ((usize, f32), bool),
    executor: BackgroundExecutor,
) -> Result<Listed, Trouble> {
    let count = screen.0;
    let folders = Arc::new(index().folders_in(Path::new(id)));
    let icloud = finder_fs::icloud::is_drive(Path::new(id));
    let read = {
        let (folders, executor) = (folders.clone(), executor.clone());
        move || read_batches(id, &folders, extras, &executor)
    };
    // A date or size sort's pick from the index waits on fseventsd, so its disk pass starts first.
    let early = (!by_name(order)).then(|| std::thread::spawn(read.clone()));
    let batches = move || early.map_or_else(read, |early| early.join().unwrap());
    let shown = |rows: Vec<Arc<Row>>, hidden: bool| {
        let lines = listing::lines_of(&rows, hidden);
        (rows, (hidden, lines))
    };
    let vouched = [Some(order.sort), order.group]
        .into_iter()
        .flatten()
        .all(|column| matches!(column, Column::Name | Column::Modified | Column::Size));
    let known = first.filter(|_| vouched && !icloud).and_then(|hidden| {
        let entries = indexed_screen(id, &folders, order, hidden, count)?;
        let rows = screen_rows(id, entries.iter(), &folders, extras, screen);
        Some(shown(rows, hidden))
    });
    if known.is_some() {
        let rest = executor.spawn(async move {
            Ok(finish(id, batches()?, None, (order, plain, extras), screen).await)
        });
        return Ok(Listed { first: known, rest });
    }
    let batches = batches()?;
    let entries: Vec<&Entry> = batches
        .iter()
        .flat_map(|(entries, _)| entries.iter())
        .collect();
    let (shared, first) = first
        .filter(|_| !icloud)
        .and_then(|hidden| {
            let chosen = first_screen(id, &entries, &folders, order, hidden, count, false)?;
            let rows = screen_rows(
                id,
                chosen.iter().map(|&at| entries[at]),
                &folders,
                extras,
                screen,
            );
            Some(((chosen, rows.clone()), (rows, hidden)))
        })
        .unzip();
    let rest = executor.spawn(async move {
        Ok(finish(id, batches, shared, (order, plain, extras), screen).await)
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
        let headed = self.headed_for() == Some(id);
        let (order, plain, extras) = match headed {
            true => (
                self.order_for(id),
                self.plain_on_arrival(),
                self.extras_in(id),
            ),
            false => (self.order(), self.plain(), self.wants_extras()),
        };
        // A disclosed folder shows its rows at once: rows landing later would push down those below.
        let first = plain.filter(|_| empty && (headed || id == self.here()));
        let screen = ((self.on_screen(), self.art_size()), extras);
        let executor = cx.background_executor().clone();
        let listed = cx
            .background_executor()
            .spawn(async move { list(id, order, (first, plain), screen, executor) });
        cx.spawn(async move |this, cx| {
            let rest = match listed.await {
                Ok(Listed { first, rest }) => {
                    if let Some((rows, lines)) = first {
                        let alive = this.update(cx, |this, cx| {
                            this.deliver(id, Some(rows), Some(lines), false, order, cx)
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
                Ok((rows, lines)) => this.deliver(id, Some(rows), lines, true, order, cx),
                Err(trouble) => this.troubled(id, trouble, fresh, order, cx),
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
        order: Order,
        cx: &mut Context<Self>,
    ) {
        match trouble {
            Trouble::Renamed(now) => self.folder_renamed(id, now, cx),
            Trouble::Gone(above) => self.folder_gone(id, above, cx),
            Trouble::Refused(error) => {
                self.deliver(id, None, None, true, order, cx);
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
        order: Order,
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
        if done && order != self.order() {
            self.sort_folder(id, self.order(), cx);
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

    fn sort_folder(&mut self, id: &'static str, by: Order, cx: &mut Context<Self>) {
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
        by: Order,
        was: usize,
        (rows, lines): Sorted,
        cx: &mut Context<Self>,
    ) {
        let now = self.order();
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

    pub(crate) fn sort_held(&mut self, by: Order, cx: &mut Context<Self>) {
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
        by: Order,
        was: &[(&'static str, usize)],
        (mut folders, shape, walk): Resorted,
        cx: &mut Context<Self>,
    ) {
        let wanted = self.view_sort(self.pending_view.unwrap_or(self.view));
        if by != wanted {
            return self.sort_held(wanted, cx);
        }
        let turned = by != self.order();
        self.set_order(by);
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
