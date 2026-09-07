//! Which rows each context shows, in the order Finder shows them.

use std::cell::Cell;
use std::collections::HashSet;

use gpui::SharedString;
use native_gpui::{MenuEntry, MenuView};

use super::{Act, Menu};
use crate::app::FinderDemo;
use crate::data::{Column, Line};
use crate::ops::clipboard::can_paste;
use crate::tags::{finder_color, tags};
use crate::view::ICON_VIEW;
use crate::views::header::column_title;

thread_local! {
    /// Which dot the tag row was clicked on, read once the row reports its pick.
    static SWATCH: Cell<Option<usize>> = const { Cell::new(None) };
}

const OPTIONAL: [Column; 3] = [Column::Modified, Column::Size, Column::Kind];

const SORTABLE: [Column; 4] = [Column::Name, Column::Kind, Column::Modified, Column::Size];

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
        MenuEntry::item(subject("Compress", &line.row.display, many, chosen)).disabled(),
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

/// The swatch the tag row reported, taken once by the pick that follows it.
pub(super) fn picked_swatch() -> Option<usize> {
    SWATCH.with(Cell::take)
}
