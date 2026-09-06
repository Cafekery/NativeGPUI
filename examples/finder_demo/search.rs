//! Finder's search field: this folder's rows filter as they always have, and the index adds what
//! it knows about the rest of the subtree.

use std::path::PathBuf;

use gpui::{Context, SharedString, Window};

use crate::data;
use crate::listing::next_rev;
use crate::{FinderDemo, index, prefs, scope};

/// Rows a search or a tag listing shows at most.
const HITS: usize = 200;

impl FinderDemo {
    pub(crate) fn searching(&self) -> bool {
        !self.query.trim().is_empty() && !self.hits.is_empty()
    }

    /// Rows found elsewhere on the disk rather than read out of this folder: the search field's
    /// hits, or a tag's files.
    pub(crate) fn results(&self) -> bool {
        self.tag_listing.is_some() || self.searching()
    }

    /// Finder's search field: this folder's rows filter as they always have, and the index adds
    /// what it knows about the rest of the subtree.
    pub(crate) fn search(&mut self, text: String, cx: &mut Context<Self>) {
        self.query = text;
        self.run_search(cx);
    }

    /// Re-runs the current query, for a fresh scope or a fresh folder.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.tag_listing = None;
        self.hits.clear();
        self.hits_rev = next_rev();
        let query = self.query.trim().to_string();
        cx.notify();
        if query.is_empty() {
            return;
        }
        let under = match self.scope {
            scope::THIS_MAC => None,
            _ => Some(PathBuf::from(self.here())),
        };
        let wanted = query.clone();
        cx.spawn(async move |this, cx| {
            let hits = cx
                .background_executor()
                .spawn(async move {
                    let index = index();
                    index
                        .search(&query, under.as_deref(), HITS)
                        .iter()
                        .map(|hit| data::hit_row(&hit.path, &hit.entry, index))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                if this.query.trim() == wanted {
                    this.hits = hits;
                    this.hits_rev = next_rev();
                    cx.notify();
                }
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
