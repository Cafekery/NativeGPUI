use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use block2::StackBlock;
use gpui::{App, Context, SharedString, Window};
use objc2_app_kit::{NSControl, NSControlTextDidEndEditingNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter};

use crate::app::FinderDemo;
use crate::chrome::toolbar::SEARCH_ID;
use crate::chrome::{criteria_bar, scope_bar};
use crate::data;
use crate::listing::next_rev;
use crate::{index, prefs};

const HITS: usize = 200;
const ALL_HITS: usize = 10_000;
const CHUNK: usize = 256;
pub(crate) const RECENTS: &str = "recents";

/// Whether the window's toolbar search field has the keyboard.
pub(crate) fn search_field_editing(window: &Window) -> bool {
    native_gpui::toolbar_item_view(window, SEARCH_ID)
        .and_then(|view| view.downcast::<NSControl>().ok())
        .is_some_and(|field| field.currentEditor().is_some())
}

/// Finder's search stays on while its field is being typed in, even empty; a field giving up the
/// keyboard may be a window's search ending.
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

impl FinderDemo {
    pub(crate) fn searching(&self) -> bool {
        !self.filtered.trim().is_empty() && !self.hits.is_empty()
    }

    pub(crate) fn in_search(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub(crate) fn search_bars_h(&self) -> f32 {
        scope_bar::SCOPE_H + criteria_bar::CRITERIA_H * self.criteria.len() as f32
    }

    pub(crate) fn results(&self) -> bool {
        self.tag_listing.is_some() || self.searching()
    }

    pub(crate) fn search(&mut self, text: String, cx: &mut Context<Self>) {
        self.arrive_now(cx);
        if self.query.trim().is_empty() && !text.trim().is_empty() {
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
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.query.clear();
        self.hits.clear();
        self.hits_rev = next_rev();
        self.tag_listing = Some(id);
        self.retitle(window);
        cx.notify();
        let tag = id.strip_prefix("tag:").unwrap_or_default().to_string();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let rows = match (id == RECENTS, crate::smart::file_of(id)) {
                        (true, _) => data::recent_rows(&crate::home(), index(), HITS),
                        (_, Some(file)) => crate::smart::rows(file, index(), HITS),
                        _ => data::tagged_rows(&tag, index(), HITS),
                    };
                    let marks: Vec<(SharedString, Vec<usize>)> = rows
                        .iter()
                        .map(|row| (row.path.clone(), crate::tags::read_row_tags(&row.path)))
                        .filter(|(_, carried)| !carried.is_empty())
                        .collect();
                    (rows, marks)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.tag_listing == Some(id) {
                    let (rows, marks) = found;
                    this.hits = rows;
                    this.hits_rev = next_rev();
                    this.tags.extend(marks);
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
