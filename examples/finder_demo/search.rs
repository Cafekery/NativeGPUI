use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use block2::StackBlock;
use gpui::{App, Context, Window};
use objc2_app_kit::{NSControl, NSControlTextDidEndEditingNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter};

use crate::app::FinderDemo;
use crate::chrome::criteria_bar::Criterion;
use crate::chrome::toolbar::SEARCH_ID;
use crate::chrome::{criteria_bar, scope_bar};
use crate::data;
use crate::data::Row;
use crate::listing::next_rev;
use crate::{fsearch, index, prefs};

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

/// Rows for fsearch's hits, in order: from what the app's own index already
/// recorded, else one getattrlist each (a few threads at once). `None` once
/// `stale` says a newer search started.
fn fsearch_rows(
    paths: &[PathBuf],
    index: &finder_fs::index::Index,
    criteria: &[Criterion],
    stale: impl Fn() -> bool + Sync,
) -> Option<Vec<Arc<Row>>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let parts = std::thread::scope(|s| {
        let tasks: Vec<_> = paths
            .chunks(paths.len().div_ceil(threads).max(1))
            .map(|part| {
                let stale = &stale;
                s.spawn(move || {
                    let mut rows = Vec::with_capacity(part.len());
                    for chunk in part.chunks(CHUNK) {
                        if stale() {
                            return None;
                        }
                        rows.extend(
                            chunk
                                .iter()
                                .filter_map(|path| {
                                    let entry = index
                                        .entry(path)
                                        .or_else(|| finder_fs::list::listed_entry_at(path).ok())?;
                                    Some(data::hit_row(path, &entry, index))
                                })
                                .filter(|row| criteria.iter().all(|it| it.holds(row))),
                        );
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

impl FinderDemo {
    pub(crate) fn in_search(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub(crate) fn search_bars_h(&self) -> f32 {
        scope_bar::SCOPE_H + criteria_bar::row_height() * self.criteria.len() as f32
    }

    pub(crate) fn results(&self) -> bool {
        self.tag_listing.is_some() || (!self.filtered.trim().is_empty() && !self.hits.is_empty())
    }

    pub(crate) fn search(&mut self, text: String, cx: &mut Context<Self>) {
        self.arrive_now(cx);
        if !self.in_search() && !text.trim().is_empty() {
            self.scope = prefs::search_scope();
        }
        self.query = text;
        self.run_search(cx);
    }

    pub(crate) fn run_search(&mut self, cx: &mut Context<Self>) {
        self.tag_listing = None;
        cx.notify();
        let live = self.search_gen.clone();
        let generation = live.fetch_add(1, Ordering::Relaxed) + 1;
        let wanted = self.query.trim().to_string();
        let criteria = self.criteria.clone();
        let everywhere = self.scope == scope_bar::THIS_MAC;
        let daemon = self.scope != scope_bar::NETWORK;
        let under: Vec<PathBuf> = match self.scope {
            scope_bar::THIS_MAC => Vec::new(),
            scope_bar::NETWORK => data::network_roots().iter().map(PathBuf::from).collect(),
            _ => vec![PathBuf::from(self.here())],
        };
        cx.spawn(async move |this, cx| {
            for limit in [HITS, ALL_HITS] {
                let query = wanted.clone();
                let (under, criteria, pass) = (under.clone(), criteria.clone(), live.clone());
                let found = cx
                    .background_executor()
                    .spawn(async move {
                        if query.is_empty() {
                            return Some((Vec::new(), true));
                        }
                        let index = index();
                        let fast = daemon
                            .then(|| fsearch::search(&query, &under, limit))
                            .flatten();
                        if let Some(paths) = fast {
                            let complete = paths.len() < limit;
                            let mut rows = fsearch_rows(&paths, index, &criteria, || {
                                pass.load(Ordering::Relaxed) != generation
                            })?;
                            rows.truncate(limit);
                            return Some((rows, complete));
                        }
                        let mut found = match everywhere {
                            true => index.search(&query, None, limit),
                            false => under
                                .iter()
                                .flat_map(|root| index.search(&query, Some(root), limit))
                                .collect(),
                        };
                        let complete = found.len() < limit;
                        found.sort_by_key(|hit| std::cmp::Reverse(hit.score));
                        let mut rows = Vec::new();
                        for hits in found.chunks(CHUNK) {
                            if pass.load(Ordering::Relaxed) != generation {
                                return None;
                            }
                            rows.extend(
                                hits.iter()
                                    .map(|hit| data::hit_row(&hit.path, &hit.entry, index))
                                    .filter(|row| criteria.iter().all(|it| it.holds(row))),
                            );
                            if rows.len() >= limit {
                                rows.truncate(limit);
                                break;
                            }
                        }
                        Some((rows, complete))
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
                if complete || !matches!(shown, Ok(true)) {
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
        self.query.clear();
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
                        (true, _) => data::recent_rows(&crate::home(), index(), HITS),
                        (_, Some(file)) => crate::smart::rows(file, index(), HITS),
                        _ => data::tagged_rows(tag, index(), HITS),
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
