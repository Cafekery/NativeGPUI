use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, SharedString, Window};

use crate::app::FinderDemo;
use crate::chrome::{criteria_bar, scope_bar};
use crate::data::{self, Row};
use crate::listing::next_rev;
use crate::{index, prefs};

const HITS: usize = 200;
const ALL_HITS: usize = usize::MAX;
pub(crate) const RECENTS: &str = "recents";

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
        self.query = text;
        self.run_search(cx);
    }

    pub(crate) fn run_search(&mut self, cx: &mut Context<Self>) {
        self.tag_listing = None;
        cx.notify();
        self.search_gen += 1;
        let generation = self.search_gen;
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
                let (under, criteria) = (under.clone(), criteria.clone());
                let (hits, complete) = cx
                    .background_executor()
                    .spawn(async move {
                        if query.is_empty() {
                            return (Vec::new(), true);
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
                        let rows = found
                            .iter()
                            .map(|hit| data::hit_row(&hit.path, &hit.entry, index))
                            .filter(|row| criteria.iter().all(|it| it.holds(row)))
                            .take(limit)
                            .collect::<Vec<_>>();
                        (rows, complete)
                    })
                    .await;
                let rev = next_rev();
                let taken = this.update(cx, |this, _| {
                    (this.search_gen == generation).then(|| {
                        (
                            this.shape_filtered(&wanted, &hits, rev),
                            this.folders.held(),
                        )
                    })
                });
                let Ok(Some((shape, held))) = taken else {
                    return;
                };
                let filtered = cx
                    .background_executor()
                    .spawn(async move {
                        let folders: HashMap<&'static str, Vec<Arc<Row>>> =
                            held.into_iter().collect();
                        let walk = shape.build(&|id| folders.get(id).map(Vec::as_slice));
                        (shape, walk)
                    })
                    .await;
                let shown = this.update(cx, |this, cx| {
                    if this.search_gen != generation {
                        return false;
                    }
                    let (shape, walk) = filtered;
                    this.filtered = wanted.clone();
                    this.hits = hits;
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
