//! Finder's search field: this folder's rows filter, and the index adds the rest of the subtree.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, SharedString, Window};

use crate::app::FinderDemo;
use crate::chrome::scope_bar;
use crate::data::{self, Row};
use crate::listing::next_rev;
use crate::{index, prefs};

/// Rows a search or a tag listing shows at most.
const HITS: usize = 200;

impl FinderDemo {
    pub(crate) fn searching(&self) -> bool {
        !self.filtered.trim().is_empty() && !self.hits.is_empty()
    }

    /// Rows found elsewhere on the disk: the search field's hits, or a tag's files.
    pub(crate) fn results(&self) -> bool {
        self.tag_listing.is_some() || self.searching()
    }

    pub(crate) fn search(&mut self, text: String, cx: &mut Context<Self>) {
        self.query = text;
        self.run_search(cx);
    }

    /// Finder filters as the field is typed into, so the rows a query names are found and their
    /// lines built off the main thread, and a keystroke that supersedes one drops it where it is.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.tag_listing = None;
        cx.notify();
        let wanted = self.query.trim().to_string();
        let under = match self.scope {
            scope_bar::THIS_MAC => None,
            _ => Some(PathBuf::from(self.here())),
        };
        cx.spawn(async move |this, cx| {
            let query = wanted.clone();
            let hits = cx
                .background_executor()
                .spawn(async move {
                    if query.is_empty() {
                        return Vec::new();
                    }
                    let index = index();
                    index
                        .search(&query, under.as_deref(), HITS)
                        .iter()
                        .map(|hit| data::hit_row(&hit.path, &hit.entry, index))
                        .collect::<Vec<_>>()
                })
                .await;
            let rev = next_rev();
            let taken = this.update(cx, |this, _| {
                (this.query.trim() == wanted).then(|| {
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
                    let folders: HashMap<&'static str, Vec<Arc<Row>>> = held.into_iter().collect();
                    let walk = shape.build(&|id| folders.get(id).map(Vec::as_slice));
                    (shape, walk)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.query.trim() != wanted {
                    return;
                }
                let (shape, walk) = filtered;
                this.filtered = wanted;
                this.hits = hits;
                this.hits_rev = rev;
                this.shaped(&shape, walk);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Finder's tag rows: the files Spotlight says carry the tag, in a listing titled with it.
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
                    let rows = data::tagged_rows(&tag, index(), HITS);
                    let marks: Vec<(SharedString, Vec<usize>)> = rows
                        .iter()
                        .map(|row| (row.path.clone(), crate::tags::row_tags(&row.path)))
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

    /// Finder's scope bar: which segment is picked, per folder for the session, everywhere after.
    pub(crate) fn set_scope(&mut self, scope: usize, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        prefs::set_search_scope(scope);
        self.run_search(cx);
    }
}
