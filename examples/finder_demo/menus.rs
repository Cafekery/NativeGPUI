//! Finder's contextual menus, read off the real one: which rows each context shows, in what order.

use std::collections::HashSet;

use std::cell::Cell;

use gpui::{ClipboardItem, Context, ElementId, IntoElement, SharedString, WeakEntity, Window};
use native_gpui::tokens::system_color;
use native_gpui::{MenuEntry, MenuView, NativeContextMenu, native_context_menu};

use crate::data::{Column, Folder, Line, Row};
use crate::header::{ColumnSpec, column_title};
use crate::ui::TAGS;
use crate::{FinderDemo, ICON_VIEW, open_finder_window};

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
    NewFolder,
    Inspector,
    Sort(Column),
    Open,
    OpenWindow,
    Trash,
    Rename,
    Duplicate,
    Copy,
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
    menu.separator();
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
            true => format!("Copy {chosen} Items"),
            false => "Copy".to_string(),
        }),
        Act::Copy,
    );
    menu.separator();
    menu.item(MenuEntry::item("Share…").disabled(), Act::Nothing);
    menu.separator();
    menu.view(
        MenuView::swatches_picking(
            TAGS.iter().map(|(_, color)| system_color(*color)),
            |index| SWATCH.with(|cell| cell.set(Some(index))),
        ),
        Act::Swatch,
    );
    menu.item(MenuEntry::item("Tags…").disabled(), Act::Nothing);
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
                Some(tag) => self.tag_selection(tag),
                None => return,
            },
            Act::NewFolder => {
                self.new_folder();
            }
            Act::NewFolderWith => self.new_folder_with_selection(),
            Act::Groups => self.grouped = !self.grouped,
            Act::ViewOptions => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::options::open(&demo, cx));
            }
            Act::Inspector => self.preview = true,
            Act::GetInfo => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, cx));
            }
            Act::Sort(column) => self.sort_by(column),
            Act::Open => self.open_here(window, cx),
            Act::OpenWindow => self.open_in_window(window, cx),
            Act::Trash => self.trash_selection(),
            Act::Rename => self.renaming = self.cursor.clone(),
            Act::Duplicate => self.duplicate_selection(),
            Act::Copy => self.copy_selection(cx),
            Act::Toggle(column) => self.toggle_column(column),
        }
        cx.notify();
    }

    pub(crate) fn new_folder(&mut self) -> Option<SharedString> {
        self.make_folder("untitled folder")
    }

    fn make_folder(&mut self, stem: &str) -> Option<SharedString> {
        let here = self.target();
        let folder = self.store.get(here)?;
        let mut name = String::from(stem);
        let mut copy = 2;
        while folder.rows.iter().any(|row| row.name.as_ref() == name) {
            name = format!("{stem} {copy}");
            copy += 1;
        }
        let title: &'static str = Box::leak(name.clone().into_boxed_str());
        let id: &'static str = Box::leak(format!("{here}/{title}").into_boxed_str());
        let crumbs: &'static [&'static str] = Box::leak(
            folder
                .crumbs
                .iter()
                .copied()
                .chain([title])
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let row = Row::folder(title, id);
        let key = SharedString::from(format!("{here}/{title}"));
        self.store.insert(
            id,
            Folder {
                title,
                parent: Some(here),
                crumbs,
                rows: Vec::new(),
            },
        );
        if let Some(folder) = self.store.get_mut(here) {
            folder.rows.push(row);
        }
        self.selected = HashSet::from([key.clone()]);
        self.cursor = Some(key.clone());
        self.anchor = Some(key.clone());
        self.renaming = Some(key.clone());
        Some(key)
    }

    /// Finder's ⌃⌘N: the chosen rows move into a folder made for them.
    pub(crate) fn new_folder_with_selection(&mut self) {
        let here = self.target();
        let chosen = self.selected.clone();
        if chosen.is_empty() {
            return;
        }
        let Some(key) = self.make_folder("New Folder With Items") else {
            return;
        };
        let into: &'static str = match self.store.get(here).and_then(|folder| {
            folder
                .rows
                .iter()
                .find(|row| SharedString::from(format!("{here}/{}", row.name)) == key)
                .and_then(|row| row.opens)
        }) {
            Some(into) => into,
            None => return,
        };
        let moved: Vec<Row> = match self.store.get_mut(here) {
            Some(folder) => {
                let (moved, kept) = folder.rows.drain(..).partition(|row| {
                    chosen.contains(&SharedString::from(format!("{here}/{}", row.name)))
                });
                folder.rows = kept;
                moved
            }
            None => return,
        };
        if let Some(folder) = self.store.get_mut(into) {
            folder.rows = moved;
        }
    }

    fn duplicate_selection(&mut self) {
        let here = self.target();
        let Some(folder) = self.store.get_mut(here) else {
            return;
        };
        let copies: Vec<Row> = folder
            .rows
            .iter()
            .filter(|row| {
                self.selected
                    .contains(&SharedString::from(format!("{here}/{}", row.name)))
            })
            .map(|row| {
                let mut copy = row.clone();
                copy.name = SharedString::from(match row.name.rsplit_once('.') {
                    Some((stem, ext)) if !stem.is_empty() => format!("{stem} copy.{ext}"),
                    _ => format!("{} copy", row.name),
                });
                copy
            })
            .collect();
        if copies.is_empty() {
            return;
        }
        let keys: Vec<SharedString> = copies
            .iter()
            .map(|row| SharedString::from(format!("{here}/{}", row.name)))
            .collect();
        folder.rows.extend(copies);
        self.selected = keys.iter().cloned().collect();
        self.cursor = keys.last().cloned();
        self.anchor = self.cursor.clone();
    }

    fn trash_selection(&mut self) {
        let here = self.target();
        let gone: HashSet<SharedString> = self.selected.clone();
        if let Some(folder) = self.store.get_mut(here) {
            folder
                .rows
                .retain(|row| !gone.contains(&SharedString::from(format!("{here}/{}", row.name))));
        }
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.row.name.to_string())
            .collect();
        if !names.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(names.join("\n")));
        }
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
    }

    fn toggle_column(&mut self, column: Column) {
        match self.list_columns.iter().position(|s| s.column == column) {
            Some(index) => {
                self.list_columns.remove(index);
            }
            None => self.list_columns.push(ColumnSpec::of(column)),
        }
        self.fit_name_column();
    }
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
