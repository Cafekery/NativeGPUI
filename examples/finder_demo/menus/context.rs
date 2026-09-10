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
    static SWATCH: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Finder's column list for a local folder, in its order; a `None` shows the way Finder shows a
/// column it cannot fill.
const COLUMNS: [(&str, Option<Column>); 11] = [
    ("Date Modified", Some(Column::Modified)),
    ("Date Created", None),
    ("Date Last Opened", None),
    ("Date Added", None),
    ("Size", Some(Column::Size)),
    ("Kind", Some(Column::Kind)),
    ("Last Modified By", None),
    ("Shared By", None),
    ("Version", None),
    ("Comments", None),
    ("Tags", None),
];

const SORTABLE: [Column; 4] = [Column::Name, Column::Kind, Column::Modified, Column::Size];

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
        let apps = crate::openwith::apps_for(Path::new(line.key.as_ref()));
        match apps.is_empty() {
            true => menu.item(MenuEntry::item("Open With").disabled(), Act::Nothing),
            false => menu.submenu(
                "Open With",
                apps.iter()
                    .map(|app| {
                        let title = match app.default {
                            true => format!("{} (default)", app.name),
                            false => app.name.clone(),
                        };
                        (
                            MenuEntry::item(title),
                            Act::OpenWith(intern(&app.path.to_string_lossy())),
                        )
                    })
                    .collect(),
            ),
        }
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
        MenuEntry::item(subject("Compress", &line.row.display, many, chosen)),
        Act::Compress,
    );
    menu.item(MenuEntry::item("Duplicate"), Act::Duplicate);
    menu.item(MenuEntry::item("Make Alias"), Act::MakeAlias);
    menu.item(MenuEntry::item("Quick Look"), Act::QuickLook);
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
    menu.item(MenuEntry::item("Share…"), Act::Share);
    menu.separator();
    // Finder names each dot for what picking it does: Remove once every chosen item carries it.
    let chosen_tags: Vec<Vec<usize>> = match selected.contains(&line.key) {
        true => selected
            .iter()
            .map(|key| crate::tags::row_tags(key))
            .collect(),
        false => vec![crate::tags::row_tags(&line.key)],
    };
    let swatches = tags().iter().enumerate().map(|(index, tag)| {
        let verb = match chosen_tags.iter().all(|carried| carried.contains(&index)) {
            true => "Remove",
            false => "Add",
        };
        (
            finder_color(tag.color),
            format!("{verb} \u{201c}{}\u{201d} label", tag.name),
        )
    });
    menu.view(
        MenuView::swatches_picking(swatches, |index| SWATCH.with(|cell| cell.set(Some(index)))),
        Act::Swatch,
    );
    menu.item(MenuEntry::item("Tags…"), Act::Tags);
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

/// Finder's menu for the kind of place the row names, and nothing at all on a group header.
pub(crate) fn sidebar(id: &SharedString) -> Menu {
    match id.strip_prefix("tag:") {
        Some(name) => tag_row(id, name),
        None if id.starts_with('/') => place_row(id),
        None => Menu::default(),
    }
}

/// Finder's menu on a tag row; the "All Tags…" row carries nothing but the first line.
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
    let mut menu = Menu::default();
    menu.item(MenuEntry::item("Open in New Tab"), Act::OpenTab(at));
    if held {
        menu.item(
            MenuEntry::item("Show in Enclosing Folder"),
            Act::Enclosing(at),
        );
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
    menu.item(MenuEntry::item("Get Info"), Act::GetInfoAt(at));
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

/// The Trash, iCloud Drive and Network are places Finder offers neither an enclosing folder nor a
/// Dock tile for.
fn holds(path: &Path) -> bool {
    !(volumes::standard(volumes::Standard::Trash).is_some_and(|trash| trash == path)
        || crate::home().join("Library/Mobile Documents/com~apple~CloudDocs") == path
        || path == Path::new("/Network"))
}

pub(crate) fn header(demo: &FinderDemo) -> Menu {
    let mut menu = Menu::default();
    for (title, column) in COLUMNS {
        match column {
            Some(column) => {
                let shown = demo.list_columns.iter().any(|spec| spec.column == column);
                menu.item(MenuEntry::item(title).checked(shown), Act::Toggle(column));
            }
            None => menu.item(MenuEntry::item(title).disabled(), Act::Nothing),
        }
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

pub(super) fn picked_swatch() -> Option<usize> {
    SWATCH.with(Cell::take)
}
