//! Finder's contextual menus: which rows each context shows, and what a pick runs.

mod actions;
mod bar;
mod context;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, ElementId, IntoElement, Pixels, Point, WeakEntity, Window};
use native_gpui::{MenuEntry, MenuView, NativeContextMenu, native_context_menu};

pub(crate) use self::actions::Act;
pub(crate) use self::bar::{
    Back, CopyNames, Cut, Forward, MoveHere, NewTab, Paste, SelectAll, ZoomIn, ZoomOut, go_rows,
    install, remember,
};
pub(crate) use self::context::{background, crumb, header, item, sidebar};
use crate::app::FinderDemo;
use crate::data::Line;

/// Hangs `menu` on `child` as its context menu, running what the pick names.
pub(crate) fn attach(
    id: impl Into<ElementId>,
    menu: Menu,
    this: WeakEntity<FinderDemo>,
    child: impl IntoElement,
) -> NativeContextMenu {
    native_context_menu(id, child)
        .entries(menu.entries)
        .on_select(picks(Rc::new(RefCell::new(menu.acts)), this))
}

/// A menu built where the right-click landed rather than on every frame: a hosted view per
/// painted row is what a long listing cannot afford.
fn menu_at(
    id: impl Into<ElementId>,
    this: WeakEntity<FinderDemo>,
    build: impl Fn(&FinderDemo, Point<Pixels>) -> Menu + 'static,
    child: impl IntoElement,
) -> NativeContextMenu {
    let acts: Rc<RefCell<Vec<Act>>> = Rc::default();
    let built = acts.clone();
    let demo = this.clone();
    native_context_menu(id, child)
        .entries_at(move |at, _, cx| {
            let menu = demo
                .update(cx, |demo: &mut FinderDemo, _| build(demo, at))
                .unwrap_or_default();
            *built.borrow_mut() = menu.acts;
            menu.entries
        })
        .on_select(picks(acts, this))
}

pub(crate) fn backdrop(
    id: impl Into<ElementId>,
    this: WeakEntity<FinderDemo>,
    child: impl IntoElement,
) -> NativeContextMenu {
    menu_at(id, this, |demo, _| background(demo), child)
}

/// One menu for a whole view, for the row `hit` names.
pub(crate) fn rows_menu(
    id: impl Into<ElementId>,
    this: WeakEntity<FinderDemo>,
    hit: impl Fn(&FinderDemo, Point<Pixels>) -> Option<Line> + 'static,
    child: impl IntoElement,
) -> NativeContextMenu {
    let build = move |demo: &FinderDemo, at| match hit(demo, at) {
        Some(line) => item(&demo.selected, &line),
        None => Menu::default(),
    };
    menu_at(id, this, build, child)
}

fn picks(
    acts: Rc<RefCell<Vec<Act>>>,
    this: WeakEntity<FinderDemo>,
) -> impl Fn(&usize, &mut Window, &mut App) {
    move |index, window, cx| {
        let act = acts.borrow().get(*index).copied().unwrap_or(Act::Nothing);
        this.update(cx, |demo: &mut FinderDemo, cx| {
            demo.run_menu(act, window, cx)
        })
        .ok();
    }
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
