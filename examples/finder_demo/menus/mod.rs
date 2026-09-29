mod actions;
mod bar;
mod context;
mod edit;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, ElementId, IntoElement, Pixels, Point, SharedString, WeakEntity, Window};
use native_gpui::{MenuEntry, MenuView, NativeContextMenu, native_context_menu};

pub(crate) use self::actions::Act;
pub(crate) use self::bar::{
    Back, CopyNames, Forward, NewTab, SelectAll, ShowClipboard, ShowProgressWindow, ZoomIn,
    ZoomOut, chosen_state, go_rows, install, remember,
};
pub(crate) use self::context::{background, header, item, sidebar};
use crate::app::FinderDemo;
use crate::data::Line;

pub(crate) fn attach(
    id: impl Into<ElementId>,
    menu: Menu,
    this: WeakEntity<FinderDemo>,
    child: impl IntoElement,
) -> NativeContextMenu {
    native_context_menu(id, child)
        .entries(menu.entries)
        .on_select(picks(Rc::new(RefCell::new(menu.acts)), Rc::default(), this))
}

pub(crate) fn crumb_menu(
    id: impl Into<ElementId>,
    target: &'static str,
    name: SharedString,
    this: WeakEntity<FinderDemo>,
    child: impl IntoElement,
) -> NativeContextMenu {
    let acts: Rc<RefCell<Vec<Act>>> = Rc::default();
    let built = acts.clone();
    native_context_menu(id, child)
        .entries_at(move |_, _, _| {
            let menu = context::crumb(target, &name);
            *built.borrow_mut() = menu.acts;
            menu.entries
        })
        .on_select(picks(acts, Rc::default(), this))
}

fn menu_at(
    id: impl Into<ElementId>,
    this: WeakEntity<FinderDemo>,
    build: impl Fn(&mut FinderDemo, Point<Pixels>) -> Menu + 'static,
    child: impl IntoElement,
) -> NativeContextMenu {
    let acts: Rc<RefCell<Vec<Act>>> = Rc::default();
    let aim: Rc<RefCell<Option<Line>>> = Rc::default();
    let (built, aimed) = (acts.clone(), aim.clone());
    let demo = this.clone();
    let closing = this.clone();
    native_context_menu(id, child)
        .entries_at(move |at, _, cx| {
            let menu = demo
                .update(cx, |demo: &mut FinderDemo, cx| {
                    let menu = build(demo, at);
                    cx.notify();
                    menu
                })
                .unwrap_or_default();
            *built.borrow_mut() = menu.acts;
            *aimed.borrow_mut() = menu.aim;
            menu.entries
        })
        .on_close(move |_, cx| {
            closing
                .update(cx, |demo: &mut FinderDemo, cx| {
                    demo.menu_row = None;
                    cx.notify();
                })
                .ok();
        })
        .on_select(picks(acts, aim, this))
}

pub(crate) fn backdrop(
    id: impl Into<ElementId>,
    this: WeakEntity<FinderDemo>,
    child: impl IntoElement,
) -> NativeContextMenu {
    menu_at(id, this, |demo, _| background(demo), child)
}

pub(crate) fn rows_menu(
    id: impl Into<ElementId>,
    this: WeakEntity<FinderDemo>,
    hit: impl Fn(&FinderDemo, Point<Pixels>) -> Option<Line> + 'static,
    child: impl IntoElement,
) -> NativeContextMenu {
    row_menu(
        id,
        this,
        hit,
        |demo, line| demo.menu_row = Some(line.key.clone()),
        child,
    )
}

pub(crate) fn tiles_menu(
    id: impl Into<ElementId>,
    this: WeakEntity<FinderDemo>,
    hit: impl Fn(&FinderDemo, Point<Pixels>) -> Option<Line> + 'static,
    child: impl IntoElement,
) -> NativeContextMenu {
    row_menu(
        id,
        this,
        hit,
        |demo, line| {
            if !demo.selected.contains(&line.key) {
                demo.pick(line, false, false);
            }
        },
        child,
    )
}

fn row_menu(
    id: impl Into<ElementId>,
    this: WeakEntity<FinderDemo>,
    hit: impl Fn(&FinderDemo, Point<Pixels>) -> Option<Line> + 'static,
    mark: impl Fn(&mut FinderDemo, &Line) + 'static,
    child: impl IntoElement,
) -> NativeContextMenu {
    let build = move |demo: &mut FinderDemo, at| match hit(demo, at) {
        Some(line) => {
            mark(demo, &line);
            let menu = item(&demo.selected, &line);
            Menu {
                aim: Some(line),
                ..menu
            }
        }
        None => Menu::default(),
    };
    menu_at(id, this, build, child)
}

fn picks(
    acts: Rc<RefCell<Vec<Act>>>,
    aim: Rc<RefCell<Option<Line>>>,
    this: WeakEntity<FinderDemo>,
) -> impl Fn(&usize, &mut Window, &mut App) {
    move |index, window, cx| {
        let act = acts.borrow().get(*index).copied().unwrap_or(Act::Nothing);
        let aim = aim.borrow().clone();
        this.update(cx, |demo: &mut FinderDemo, cx| {
            if let Some(line) = aim.filter(|line| !demo.selected.contains(&line.key)) {
                demo.pick(&line, false, false);
            }
            demo.run_menu(act, window, cx)
        })
        .ok();
    }
}

/// A pick reports its depth-first position, so entries and acts must stay in step.
#[derive(Default)]
pub(crate) struct Menu {
    pub(crate) entries: Vec<MenuEntry>,
    pub(crate) acts: Vec<Act>,
    pub(crate) aim: Option<Line>,
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
        self.nest(MenuEntry::item(title), rows);
    }

    fn alternate_submenu(&mut self, title: &str, rows: Vec<(MenuEntry, Act)>) {
        self.nest(MenuEntry::item(title).option_alternate(), rows);
    }

    fn nest(&mut self, row: MenuEntry, rows: Vec<(MenuEntry, Act)>) {
        let (entries, acts): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
        self.entries.push(row.submenu(entries));
        self.acts.push(Act::Nothing);
        self.acts.extend(acts);
    }
}
