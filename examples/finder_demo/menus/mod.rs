//! Finder's contextual menus, read off the real one: which rows each context shows, in what
//! order, and what a pick runs.

mod actions;
mod bar;
mod context;

use std::collections::HashSet;

use gpui::{ElementId, IntoElement, SharedString, WeakEntity};
use native_gpui::{MenuEntry, MenuView, NativeContextMenu, native_context_menu};

pub(crate) use self::actions::Act;
pub(crate) use self::bar::{
    Back, CopyNames, Cut, Forward, MoveHere, NewTab, Paste, SelectAll, ZoomIn, ZoomOut, go_rows,
    install,
};
pub(crate) use self::context::{background, crumb, header, item};
use crate::app::FinderDemo;
use crate::data::Line;

/// Hangs `menu` on `child` as its context menu, running what the pick names.
pub(crate) fn attach(
    id: impl Into<ElementId>,
    menu: Menu,
    this: WeakEntity<FinderDemo>,
    child: impl IntoElement,
) -> NativeContextMenu {
    let acts = menu.acts;
    native_context_menu(id, child)
        .entries(menu.entries)
        .on_select(move |index, window, cx| {
            let act = acts.get(*index).copied().unwrap_or(Act::Nothing);
            this.update(cx, |demo: &mut FinderDemo, cx| {
                demo.run_menu(act, window, cx)
            })
            .ok();
        })
}

/// The menu for one row, built from the demo as it stands when the row is drawn.
pub(crate) fn row_menu(
    id: impl Into<ElementId>,
    line: &Line,
    selected: &HashSet<SharedString>,
    this: WeakEntity<FinderDemo>,
    child: impl IntoElement,
) -> NativeContextMenu {
    attach(id, item(selected, line), this, child)
}

/// Entries and their actions in step, since a pick reports its depth-first position.
#[derive(Default)]
pub(crate) struct Menu {
    pub(crate) entries: Vec<MenuEntry>,
    pub(crate) acts: Vec<Act>,
}

impl Menu {
    fn item(&mut self, entry: MenuEntry, act: Act) {
        self.entries.push(entry);
        self.acts.push(act);
    }

    fn separator(&mut self) {
        self.item(MenuEntry::separator(), Act::Nothing);
    }

    fn view(&mut self, view: MenuView, act: Act) {
        self.item(MenuEntry::view(view), act);
    }

    fn submenu(&mut self, title: &str, rows: Vec<(MenuEntry, Act)>) {
        let (entries, acts): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
        self.entries.push(MenuEntry::item(title).submenu(entries));
        self.acts.push(Act::Nothing);
        self.acts.extend(acts);
    }
}
