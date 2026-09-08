//! Which rows each context shows, in the order Finder shows them.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::Path;

use finder_fs::volumes;
use gpui::SharedString;
use native_gpui::{MenuEntry, MenuView};

use super::{Act, Menu};
use crate::app::FinderDemo;
use crate::data::{Column, Line, intern};
use crate::ops::clipboard::can_paste;
use crate::tags::{finder_color, finder_color_rgb, tags};
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

/// A right-click on a sidebar row: Finder's menu for the kind of place the row names, and
/// nothing at all on a group header.
pub(crate) fn sidebar(id: &SharedString) -> Menu {
    match id.strip_prefix("tag:") {
        Some(name) => tag_row(id, name),
        None if id.starts_with('/') => place_row(id),
        None => Menu::default(),
    }
}

/// Finder's menu on a tag row: what it opens, what it forgets, and the colour it wears. The
/// "All Tags…" row carries nothing but the first line.
fn tag_row(id: &SharedString, name: &str) -> Menu {
    let mut menu = Menu::default();
    let at = intern(id);
    menu.item(MenuEntry::item("Open in New Tab"), Act::OpenTab(at));
    if name.is_empty() {
        return menu;
    }
    menu.separator();
    menu.item(
        MenuEntry::item("Remove from Sidebar"),
        Act::RemoveFromSidebar(at),
    );
    menu.item(
        MenuEntry::item(format!("Delete Tag \u{201c}{name}\u{201d}\u{2026}")).disabled(),
        Act::Nothing,
    );
    menu.separator();
    menu.item(
        MenuEntry::item(format!("Rename \u{201c}{name}\u{201d}")).disabled(),
        Act::Nothing,
    );
    menu.separator();
    menu.item(MenuEntry::item("No Color").disabled(), Act::Nothing);
    for tag in tags() {
        let entry = MenuEntry::item(tag.name.clone()).disabled();
        menu.item(
            match finder_color_rgb(tag.color) {
                Some(color) => entry.dot(color),
                None => entry,
            },
            Act::Nothing,
        );
    }
    menu
}

/// Finder's menu on the rest of the sidebar: a folder, a volume, or a place that is neither.
fn place_row(id: &SharedString) -> Menu {
    let at = intern(id);
    let path = Path::new(at);
    let volume = volumes::mounted()
        .into_iter()
        .find(|volume| volume.path == path);
    let name = match &volume {
        Some(volume) => volume.name.clone(),
        None => path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
    };
    let held = holds(path);
    // The boot volume is no entry of a folder, so the demo has no line of it to show or read.
    let named = path.file_name().is_some();
    let mut menu = Menu::default();
    menu.item(MenuEntry::item("Open in New Tab"), Act::OpenTab(at));
    if held {
        let enclosing = MenuEntry::item("Show in Enclosing Folder");
        match named {
            true => menu.item(enclosing, Act::Enclosing(at)),
            false => menu.item(enclosing.disabled(), Act::Nothing),
        }
    }
    if volume.as_ref().is_some_and(|volume| volume.ejectable) {
        menu.separator();
        menu.item(
            MenuEntry::item(format!("Eject \u{201c}{name}\u{201d}")),
            Act::Eject(at),
        );
        menu.separator();
        menu.item(MenuEntry::item("Erase Disk…").disabled(), Act::Nothing);
    }
    menu.separator();
    menu.item(
        MenuEntry::item("Remove from Sidebar"),
        Act::RemoveFromSidebar(at),
    );
    menu.separator();
    let info = MenuEntry::item("Get Info");
    match named {
        true => menu.item(info, Act::GetInfoAt(at)),
        false => menu.item(info.disabled(), Act::Nothing),
    }
    if path == Path::new("/") {
        menu.item(MenuEntry::item("Manage Storage…").disabled(), Act::Nothing);
    }
    let rename = MenuEntry::item(format!("Rename \u{201c}{name}\u{201d}"));
    if volume.is_some() {
        menu.item(rename.disabled(), Act::Nothing);
    } else if crate::chrome::sidebar::pinned(at) {
        menu.item(rename, Act::RenameAt(at));
    }
    if held {
        menu.item(MenuEntry::item("Add to Dock").disabled(), Act::Nothing);
    }
    menu
}

/// Whether a folder holds the row: the Trash, iCloud Drive and Network are places Finder offers
/// neither an enclosing folder nor a Dock tile for.
fn holds(path: &Path) -> bool {
    !(volumes::standard(volumes::Standard::Trash).is_some_and(|trash| trash == path)
        || crate::home().join("Library/Mobile Documents/com~apple~CloudDocs") == path
        || path == Path::new("/Network"))
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
