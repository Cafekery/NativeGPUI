//! Finder's contextual menus, read off the real one: which rows each context shows, in what order.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;

use finder_fs::ops::{Conflict, Op};
use gpui::{Context, ElementId, IntoElement, SharedString, WeakEntity, Window};
use native_gpui::{MenuEntry, MenuView, NativeContextMenu, native_context_menu};

use crate::app::FinderDemo;
use crate::clipboard::can_paste;
use crate::data::{Column, Line};
use crate::header::{ColumnSpec, column_title};
use crate::open_finder_window;
use crate::ops::After;
use crate::tags::{finder_color, tags};
use crate::view::ICON_VIEW;

const OPTIONAL: [Column; 3] = [Column::Modified, Column::Size, Column::Kind];
const SORTABLE: [Column; 4] = [Column::Name, Column::Kind, Column::Modified, Column::Size];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Act {
    Nothing,
    Swatch,
    NewFolderWith,
    Groups,
    ViewOptions,
    GetInfo,
    GetInfoAt(&'static str),
    NewFolder,
    Inspector,
    InspectorAt(&'static str),
    Sort(Column),
    Open,
    OpenWindow,
    Trash,
    Rename,
    Duplicate,
    Copy,
    Cut,
    Paste,
    NewFile,
    OpenTerminal(&'static str),
    OpenTab(&'static str),
    OpenWindowAt(&'static str),
    Enclosing(&'static str),
    CopyPathname(&'static str),
    Toggle(Column),
}

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

thread_local! {
    /// Which dot the tag row was clicked on, read once the row reports its pick.
    static SWATCH: Cell<Option<usize>> = const { Cell::new(None) };
}

/// A right-click on empty space.
pub(crate) fn background(demo: &FinderDemo) -> Menu {
    let mut menu = Menu::default();
    menu.item(MenuEntry::item("New Folder"), Act::NewFolder);
    menu.item(MenuEntry::item("New Text File"), Act::NewFile);
    menu.separator();
    if can_paste() {
        menu.item(MenuEntry::item("Paste Item"), Act::Paste);
        menu.separator();
    }
    menu.item(MenuEntry::item("Show Inspector"), Act::Inspector);
    menu.separator();
    menu.item(
        MenuEntry::item("Use Groups").checked(demo.grouped),
        Act::Groups,
    );
    menu.submenu("Sort By", sort_rows(demo));
    if demo.view == ICON_VIEW {
        menu.item(MenuEntry::item("Clean Up"), Act::Sort(Column::Name));
        menu.submenu("Clean Up By", sort_rows(demo));
    }
    menu.item(MenuEntry::item("Show View Options"), Act::ViewOptions);
    menu.separator();
    menu.item(
        MenuEntry::item("Open in Terminal"),
        Act::OpenTerminal(demo.target()),
    );
    menu
}

/// A right-click on a row: Finder names the row, or counts the selection when the row is in it.
pub(crate) fn item(selected: &HashSet<SharedString>, line: &Line) -> Menu {
    let chosen = selected.len();
    let many = chosen > 1 && selected.contains(&line.key);
    let folder = line.row.opens.is_some();
    let mut menu = Menu::default();
    if many {
        menu.item(
            MenuEntry::item(format!("New Folder with Selection ({chosen} Items)")),
            Act::NewFolderWith,
        );
    }
    if folder {
        menu.item(MenuEntry::item(plural("Open in New Tab", many)), Act::Open);
        menu.item(
            MenuEntry::item(plural("Open in New Window", many)),
            Act::OpenWindow,
        );
    } else {
        menu.item(MenuEntry::item("Open"), Act::Open);
        menu.item(MenuEntry::item("Open With").disabled(), Act::Nothing);
    }
    if let Some(path) = line.row.opens {
        menu.item(MenuEntry::item("Open in Terminal"), Act::OpenTerminal(path));
    }
    menu.separator();
    menu.item(MenuEntry::item("Move to Trash"), Act::Trash);
    menu.separator();
    menu.item(MenuEntry::item("Get Info"), Act::GetInfo);
    menu.item(
        MenuEntry::item(if many { "Rename…" } else { "Rename" }),
        Act::Rename,
    );
    menu.item(
        MenuEntry::item(subject("Compress", &line.row.name, many, chosen)).disabled(),
        Act::Nothing,
    );
    menu.item(MenuEntry::item("Duplicate"), Act::Duplicate);
    menu.item(MenuEntry::item("Make Alias").disabled(), Act::Nothing);
    menu.item(MenuEntry::item("Quick Look").disabled(), Act::Nothing);
    menu.item(
        MenuEntry::item(match many {
            true => format!("Cut {chosen} Items"),
            false => "Cut".to_string(),
        }),
        Act::Cut,
    );
    menu.item(
        MenuEntry::item(match many {
            true => format!("Copy {chosen} Items"),
            false => "Copy".to_string(),
        }),
        Act::Copy,
    );
    if can_paste() {
        menu.item(MenuEntry::item("Paste Item"), Act::Paste);
    }
    menu.separator();
    menu.item(MenuEntry::item("Share…").disabled(), Act::Nothing);
    menu.separator();
    menu.view(
        MenuView::swatches_picking(tags().iter().map(|tag| finder_color(tag.color)), |index| {
            SWATCH.with(|cell| cell.set(Some(index)))
        }),
        Act::Swatch,
    );
    menu.item(MenuEntry::item("Tags…").disabled(), Act::Nothing);
    menu
}

/// A right-click on a path-bar crumb: everything acts on that folder, not on the selection.
pub(crate) fn crumb(id: &'static str, name: &str) -> Menu {
    let mut menu = Menu::default();
    menu.item(MenuEntry::item("Open in New Tab"), Act::OpenTab(id));
    menu.item(MenuEntry::item("Open in New Window"), Act::OpenWindowAt(id));
    menu.item(
        MenuEntry::item("Show in Enclosing Folder"),
        Act::Enclosing(id),
    );
    menu.item(MenuEntry::item("Get Info"), Act::GetInfoAt(id));
    menu.item(MenuEntry::item("Show Inspector"), Act::InspectorAt(id));
    menu.item(
        MenuEntry::item(format!("Copy “{name}” as Pathname")),
        Act::CopyPathname(id),
    );
    menu.item(MenuEntry::item("Open in Terminal"), Act::OpenTerminal(id));
    menu
}

/// The list header's menu: the optional columns, checked when they are shown.
pub(crate) fn header(demo: &FinderDemo) -> Menu {
    let mut menu = Menu::default();
    for column in OPTIONAL {
        let shown = demo.list_columns.iter().any(|spec| spec.column == column);
        menu.item(
            MenuEntry::item(column_title(column)).checked(shown),
            Act::Toggle(column),
        );
    }
    menu
}

impl FinderDemo {
    pub(crate) fn run_menu(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Nothing => return,
            Act::Swatch => match SWATCH.with(|cell| cell.take()) {
                Some(tag) => self.tag_selection(tag, cx),
                None => return,
            },
            Act::NewFolder => self.new_folder(cx),
            Act::NewFolderWith => self.new_folder_with_selection(cx),
            Act::Groups => self.grouped = !self.grouped,
            Act::ViewOptions => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::options::open(&demo, cx));
            }
            Act::Inspector => {
                self.preview = true;
                self.inspecting = None;
            }
            Act::InspectorAt(id) => {
                self.preview = true;
                self.inspecting = crate::data::path_line(id);
            }
            Act::GetInfo => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, None, cx));
            }
            Act::GetInfoAt(id) => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, Some(id), cx));
            }
            Act::Sort(column) => self.sort_by(column),
            Act::Open => self.open_here(window, cx),
            Act::OpenWindow => self.open_in_window(window, cx),
            Act::Trash => self.trash_selection(cx),
            Act::Rename => self.renaming = self.cursor.clone(),
            Act::Duplicate => self.duplicate_selection(cx),
            Act::Copy => self.copy_selection(cx),
            Act::Cut => self.cut_selection(cx),
            Act::Paste => self.paste(false, cx),
            Act::NewFile => self.new_file(cx),
            Act::OpenTerminal(path) => open_terminal(path),
            Act::OpenTab(id) => self.open_tab(id, window, cx),
            Act::OpenWindowAt(id) => {
                open_finder_window(id, 1, cx);
            }
            Act::Enclosing(id) => self.show_enclosing(id, window, cx),
            Act::CopyPathname(id) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.to_string()))
            }
            Act::Toggle(column) => self.toggle_column(column),
        }
        cx.notify();
    }

    /// Finder's New Folder: `untitled folder` in the folder on screen, named as it lands.
    pub(crate) fn new_folder(&mut self, cx: &mut Context<Self>) {
        self.make_folder("untitled folder", Vec::new(), cx);
    }

    /// Finder's ⌃⌘N: the chosen rows move into a folder made for them.
    pub(crate) fn new_folder_with_selection(&mut self, cx: &mut Context<Self>) {
        let chosen = self.paths();
        if chosen.is_empty() {
            return;
        }
        self.make_folder("New Folder With Items", chosen, cx);
    }

    /// File ▸ New Text File: `untitled.txt` in the folder on screen, named as it lands.
    pub(crate) fn new_file(&mut self, cx: &mut Context<Self>) {
        let op = Op::NewFile {
            into: PathBuf::from(self.target()),
            name: "untitled.txt".to_owned(),
        };
        self.run_op(op, After::Rename, cx);
    }

    fn make_folder(&mut self, stem: &str, with: Vec<PathBuf>, cx: &mut Context<Self>) {
        let op = Op::NewFolder {
            into: PathBuf::from(self.target()),
            name: stem.to_owned(),
        };
        if with.is_empty() {
            self.run_op(op, After::Rename, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move { finder_fs::ops::perform(op, Conflict::KeepBoth, |_| {}) })
                .await;
            let Some(folder) = made.ok().and_then(|done| done.results.first().cloned()) else {
                return;
            };
            this.update(cx, |this: &mut FinderDemo, cx| {
                this.run_op(
                    Op::Move {
                        items: with,
                        into: folder,
                    },
                    After::Nothing,
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    /// The chosen rows, as the paths an operation takes.
    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| PathBuf::from(line.key.as_ref()))
            .collect()
    }

    fn duplicate_selection(&mut self, cx: &mut Context<Self>) {
        let items = self.paths();
        if !items.is_empty() {
            self.run_op(Op::Duplicate(items), After::Select, cx);
        }
    }

    pub(crate) fn trash_selection(&mut self, cx: &mut Context<Self>) {
        let items = self.paths();
        if items.is_empty() {
            return;
        }
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.run_op(Op::Trash(items), After::Nothing, cx);
    }

    pub(crate) fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let items = self.paths();
        if items.is_empty() {
            return;
        }
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.run_op(Op::DeleteImmediately(items), After::Nothing, cx);
    }

    fn open_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let opens: Vec<&'static str> = self
            .focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .filter_map(|line| line.row.opens)
            .collect();
        if let Some(first) = opens.first() {
            self.go(first, window, cx);
        }
    }

    /// A second window, handed to AppKit as a tab of this one.
    pub(crate) fn open_tab(&self, id: &'static str, window: &Window, cx: &mut Context<Self>) {
        let host = window.window_handle();
        cx.defer(move |cx| {
            let tab = open_finder_window(id, 1, cx);
            tab.update(cx, |_, tab, cx| native_gpui::add_tab(host, tab, cx))
                .ok();
        });
    }

    fn open_in_window(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let opens: Vec<&'static str> = self
            .focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .filter_map(|line| line.row.opens)
            .collect();
        for (extra, id) in opens.into_iter().enumerate() {
            open_finder_window(id, extra + 1, cx);
        }
    }

    pub(crate) fn sort_by(&mut self, column: Column) {
        if self.sort == column {
            self.ascending = !self.ascending;
        } else {
            self.sort = column;
            self.ascending = true;
        }
        self.resort();
    }

    fn toggle_column(&mut self, column: Column) {
        match self.list_columns.iter().position(|s| s.column == column) {
            Some(index) => {
                self.list_columns.remove(index);
            }
            None => self.list_columns.push(ColumnSpec::of(column)),
        }
        self.refit_name_column();
    }
}

/// The folder context menu's Open in Terminal: the user's `TERM_PROGRAM`, if a GUI launch
/// happens to carry one, otherwise Terminal.app.
fn open_terminal(path: &str) {
    let app = std::env::var("TERM_PROGRAM").unwrap_or_else(|_| "Terminal".to_owned());
    let _ = std::process::Command::new("open")
        .args(["-a", &app])
        .arg(path)
        .spawn();
}

fn plural(title: &str, many: bool) -> String {
    match many {
        true => format!("{title}s"),
        false => title.to_string(),
    }
}

fn subject(verb: &str, name: &SharedString, many: bool, chosen: usize) -> String {
    match many {
        true => format!("{verb} {chosen} Items"),
        false => format!("{verb} “{name}”"),
    }
}

fn sort_rows(demo: &FinderDemo) -> Vec<(MenuEntry, Act)> {
    SORTABLE
        .iter()
        .map(|column| {
            (
                MenuEntry::item(column_title(*column)).checked(demo.sort == *column),
                Act::Sort(*column),
            )
        })
        .collect()
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
