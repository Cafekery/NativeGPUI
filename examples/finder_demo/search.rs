use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use block2::StackBlock;
use finder_fs::sort::fold;
use gpui::{App, Context, Window};
use objc2_app_kit::{NSControl, NSControlTextDidEndEditingNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter};

use crate::app::FinderDemo;
use crate::chrome::criteria_bar::{self, Criterion};
use crate::chrome::scope_bar;
use crate::chrome::toolbar::SEARCH_ID;
use crate::data::{self, Row};
use crate::listing::next_rev;
use crate::terms::Search;
use crate::{fsearch, prefs};

const HITS: usize = 200;
const ALL_HITS: usize = 10_000;
const CHUNK: usize = 256;
pub(crate) const RECENTS: &str = "recents";

fn search_field_editing(window: &Window) -> bool {
    native_gpui::toolbar_item_view(window, SEARCH_ID)
        .and_then(|view| view.downcast::<NSControl>().ok())
        .is_some_and(|field| field.currentEditor().is_some())
}

pub(crate) fn follow_search_fields(cx: &mut App) {
    let app = cx.to_async();
    let block = StackBlock::new(move |_: NonNull<NSNotification>| {
        app.spawn(async move |cx| {
            cx.update(|cx| {
                for handle in cx.windows() {
                    let Some(demo) = handle.downcast::<FinderDemo>() else {
                        continue;
                    };
                    demo.update(cx, |demo, window, cx| {
                        if demo.search_editing && !search_field_editing(window) {
                            demo.search_editing = false;
                            cx.notify();
                        }
                    })
                    .ok();
                }
            })
        })
        .detach();
    })
    .copy();
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSControlTextDidEndEditingNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(observer);
}

fn hit_rows(
    paths: &[&Path],
    admit: impl Fn(&Row) -> bool + Sync,
    extras: bool,
    stale: &(impl Fn() -> bool + Sync),
) -> Option<Vec<Arc<Row>>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let parts = std::thread::scope(|s| {
        let tasks: Vec<_> = paths
            .chunks(paths.len().div_ceil(threads).max(1))
            .map(|part| {
                let admit = &admit;
                s.spawn(move || {
                    let mut rows = Vec::with_capacity(part.len());
                    for chunk in part.chunks(CHUNK) {
                        if stale() {
                            return None;
                        }
                        let from = rows.len();
                        rows.extend(
                            chunk
                                .iter()
                                .filter_map(|&path| {
                                    let entry = finder_fs::list::listed_entry_at(path).ok()?;
                                    Some(data::hit_row(path, &entry))
                                })
                                .filter(|row| admit(row)),
                        );
                        if extras {
                            data::read_extras(&mut rows[from..]);
                        }
                    }
                    Some(rows)
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Option<Vec<_>>>()
    })?;
    Some(parts.concat())
}

fn spotlight(
    query: &str,
    roots: &[PathBuf],
    limit: usize,
    stale: &impl Fn() -> bool,
) -> Option<Vec<PathBuf>> {
    let mut found = Vec::new();
    for root in roots
        .iter()
        .map(Some)
        .chain(roots.is_empty().then_some(None))
    {
        let mut command = Command::new("/usr/bin/mdfind");
        if let Some(root) = root {
            command.arg("-onlyin").arg(root);
        }
        let Ok(mut child) = command
            .arg(query)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if found.len() >= limit || stale() {
                    break;
                }
                found.push(PathBuf::from(line));
            }
        }
        child.kill().ok();
        child.wait().ok();
    }
    (!stale()).then(|| fsearch::reachable_only(found))
}

fn gather(
    search: &Search,
    under: &[PathBuf],
    limit: usize,
    spotlit: bool,
    criteria: &[Criterion],
    extras: bool,
    stale: &(impl Fn() -> bool + Sync),
) -> Option<(Vec<Arc<Row>>, bool)> {
    let matcher = search.matcher();
    let mut contents: HashMap<String, HashSet<PathBuf>> = HashMap::new();
    let mut found = Vec::new();
    for word in search.contents() {
        let mut paths = fsearch::search(&search.content(&word), under, limit).unwrap_or_default();
        if let Some(query) = search.spotlight(&word).filter(|_| spotlit) {
            paths.extend(spotlight(&query, under, limit, stale)?);
        }
        found.extend(paths.iter().cloned());
        contents.entry(fold(&word)).or_default().extend(paths);
    }
    let (named, found) = match search.tagged() {
        Some(query) => (Vec::new(), spotlight(&query, under, limit, stale)?),
        None => (fsearch::search(&search.names(), under, limit)?, found),
    };
    let mut seen = HashSet::new();
    let named: Vec<&Path> = named
        .iter()
        .map(PathBuf::as_path)
        .filter(|path| seen.insert(*path))
        .collect();
    let found: Vec<&Path> = found
        .iter()
        .map(PathBuf::as_path)
        .filter(|path| seen.insert(*path))
        .collect();
    let complete = named.len() < limit && found.len() < limit;
    let criteria = |row: &Row| criteria.iter().all(|it| it.holds(row));
    let mut rows = hit_rows(
        &named,
        |row| matcher.fits(row) && criteria(row),
        extras,
        stale,
    )?;
    let held = |row: &Row, word: &str| {
        contents
            .get(word)
            .is_some_and(|paths| paths.contains(Path::new(row.path.as_ref())))
    };
    rows.extend(hit_rows(
        &found,
        |row| matcher.admits_found(row, |word| held(row, word)) && criteria(row),
        extras,
        stale,
    )?);
    rows.truncate(limit);
    Some((rows, complete))
}

impl FinderDemo {
    pub(crate) fn in_search(&self) -> bool {
        !self.query.is_empty()
    }

    pub(crate) fn search_bars_h(&self) -> f32 {
        scope_bar::SCOPE_H + criteria_bar::row_height() * self.criteria.len() as f32
    }

    pub(crate) fn results(&self) -> bool {
        self.tag_listing.is_some() || (!self.filtered.is_empty() && !self.hits.is_empty())
    }

    pub(crate) fn flat_results(&self) -> bool {
        self.in_search() || self.results()
    }

    /// `raw` is Spotlight's query for the same search, which a saved search keeps.
    pub(crate) fn search(&mut self, search: Search, raw: Option<String>, cx: &mut Context<Self>) {
        self.raw_query = raw;
        if search == self.query {
            return;
        }
        self.arrive_now(cx);
        let searching = self.in_search();
        if !searching && !search.is_empty() {
            self.scope = prefs::search_scope();
            if crate::remote::is_remote(self.here()) {
                self.scope = scope_bar::THIS_FOLDER;
            }
        }
        self.query = search;
        if self.in_search() != searching {
            let read = self.wants_extras();
            self.load_columns();
            if !read && self.wants_extras() {
                self.sort_held(self.view_sort(self.view), cx);
            }
        }
        self.run_search(cx);
    }

    pub(crate) fn run_search(&mut self, cx: &mut Context<Self>) {
        self.tag_listing = None;
        cx.notify();
        let live = self.search_gen.clone();
        let generation = live.fetch_add(1, Ordering::Relaxed) + 1;
        let wanted = self.query.clone();
        let criteria = self.criteria.clone();
        let extras = self.wants_extras();
        let indexed = self.scope != scope_bar::NETWORK;
        let under: Vec<PathBuf> = match self.scope {
            scope_bar::THIS_MAC => Vec::new(),
            _ => vec![PathBuf::from(self.here())],
        };
        let here = self.here();
        let cached = crate::remote::is_remote(here).then(|| {
            self.folder()
                .map_or_else(Arc::default, |folder| folder.rows.clone())
        });
        let spotlit =
            cached.is_none() && (!wanted.contents().is_empty() || wanted.tagged().is_some());
        cx.spawn(async move |this, cx| {
            for (limit, last) in [(HITS, false), (ALL_HITS, true)] {
                let search = wanted.clone();
                let (under, criteria, pass) = (under.clone(), criteria.clone(), live.clone());
                let cached = cached.clone();
                let found = cx
                    .background_executor()
                    .spawn(async move {
                        if search.is_empty() || !indexed {
                            return Some((Vec::new(), true));
                        }
                        let stale = || pass.load(Ordering::Relaxed) != generation;
                        if let Some(rows) = cached {
                            let matcher = search.matcher();
                            let admits = |row: &Row| {
                                matcher.admits(row) && criteria.iter().all(|it| it.holds(row))
                            };
                            if !last {
                                let shallow = rows.iter().filter(|row| admits(row)).cloned();
                                return Some((shallow.collect(), false));
                            }
                            return crate::remote::find(here, &admits, limit, &stale)
                                .map(|rows| (rows, true));
                        }
                        let gathered =
                            || gather(&search, &under, limit, last, &criteria, extras, &stale);
                        let mut hits = gathered();
                        if hits.is_none() && fsearch::loading() && !stale() {
                            if !last {
                                return Some((Vec::new(), false));
                            }
                            if fsearch::wait_ready_unless(stale) {
                                hits = gathered();
                            }
                        }
                        hits.or_else(|| (!stale()).then(|| (Vec::new(), true)))
                    })
                    .await;
                let Some((hits, complete)) = found else {
                    return;
                };
                let rev = next_rev();
                let taken = this.update(cx, |this, _| {
                    (live.load(Ordering::Relaxed) == generation).then(|| {
                        (
                            this.shape_filtered(&wanted, hits, rev),
                            this.folders.shared(),
                        )
                    })
                });
                let Ok(Some((shape, held))) = taken else {
                    return;
                };
                let filtered = cx
                    .background_executor()
                    .spawn(async move {
                        let walk = shape.build(&|id| held.get(id).map(|rows| rows.as_slice()));
                        (shape, walk)
                    })
                    .await;
                let shown = this.update(cx, |this, cx| {
                    if live.load(Ordering::Relaxed) != generation {
                        return false;
                    }
                    let (mut shape, walk) = filtered;
                    this.filtered = wanted.clone();
                    this.hits = shape.take_hits();
                    this.hits_rev = rev;
                    this.shaped(&shape, walk);
                    cx.notify();
                    true
                });
                if (complete && !spotlit) || !matches!(shown, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn show_tag(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.arriving = None;
        self.deselect();
        crate::search_field::clear(window);
        let searching = self.in_search();
        self.query = Search::default();
        if searching {
            self.load_columns();
        }
        self.criteria.clear();
        self.hits.clear();
        self.hits_rev = next_rev();
        self.tag_listing = Some(id);
        self.follow_status_bar();
        self.retitle(window);
        cx.notify();
        let tag = id.strip_prefix("tag:").unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    match (id == RECENTS, crate::smart::file_of(id)) {
                        (true, _) => data::recent_rows(&crate::home(), HITS),
                        (_, Some(file)) => crate::smart::rows(file, HITS),
                        _ => data::tagged_rows(tag, HITS),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.tag_listing == Some(id) {
                    this.hits = found;
                    this.hits_rev = next_rev();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn set_scope(&mut self, scope: usize, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        prefs::set_search_scope(scope);
        self.run_search(cx);
    }
}
