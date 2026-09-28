use std::cell::Cell;
use std::collections::HashSet;
use std::path::Path;

use finder_fs::kind::is_package_name;
use finder_fs::volumes;
use gpui::{Modifiers, SharedString};
use native_gpui::{MenuEntry, MenuView};

use super::actions::original_of;
use super::{Act, Menu};
use crate::app::FinderDemo;
use crate::data::{Column, Line, intern, intern_path, is_trash};
use crate::ops::clipboard::can_paste;
use crate::tags::{finder_color, finder_color_rgb, tags};
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};
use crate::views::header::{ColumnSpec, column_title};

thread_local! {
    static SWATCH: Cell<Option<usize>> = const { Cell::new(None) };
}

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

const VIEWS: [(&str, usize); 4] = [
    ("as Icons", ICON_VIEW),
    ("as List", LIST_VIEW),
    ("as Columns", COLUMN_VIEW),
    ("as Gallery", GALLERY_VIEW),
];

pub(crate) fn background(demo: &FinderDemo) -> Menu {
    let mut menu = Menu::default();
    menu.item(MenuEntry::item("New Folder"), Act::NewFolder);
    if can_paste() {
        menu.item(MenuEntry::item("Paste Item"), Act::Paste);
    }
    menu.separator();
    menu.item(MenuEntry::item("Get Info"), Act::GetInfoAt(demo.target()));
    menu.item(
        MenuEntry::item("Show Inspector").option_alternate(),
        Act::Inspector,
    );
    menu.separator();
    menu.submenu("View", view_rows(demo));
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

pub(crate) fn item(selected: &HashSet<SharedString>, line: &Line) -> Menu {
    let chosen = selected.len();
    let many = chosen > 1 && selected.contains(&line.key);
    let symlink = line.row.uti == "public.symlink";
    let original = symlink || line.row.uti == "com.apple.alias-file";
    let folder = folder_behind(line);
    let mut menu = Menu::default();
    if many {
        menu.item(
            MenuEntry::item(format!("New Folder with Selection ({chosen} Items)")),
            Act::NewFolderWith,
        );
    }
    match folder {
        Some(folder) => {
            let (tab, window) = match many {
                true => (Act::OpenTabs, Act::OpenWindow),
                false => (Act::OpenTab(folder), Act::OpenWindowAt(folder)),
            };
            menu.item(MenuEntry::item(plural("Open in New Tab", many)), tab);
            menu.item(
                MenuEntry::item(plural("Open in New Window", many)).option_alternate(),
                window,
            );
        }
        // Finder gives every symlink the folder rows, greyed unless it leads to a folder.
        None if symlink => {
            menu.item(MenuEntry::item("Open in New Tab").disabled(), Act::Nothing);
            menu.item(
                MenuEntry::item("Open in New Window")
                    .disabled()
                    .option_alternate(),
                Act::Nothing,
            );
        }
        None => {
            menu.item(MenuEntry::item("Open"), Act::Open);
            if !original {
                open_with(&mut menu, line);
            }
        }
    }
    if original {
        menu.item(MenuEntry::item("Show Original"), Act::ShowOriginal);
    }
    if let Some(folder) = folder.filter(|_| can_paste()) {
        menu.item(MenuEntry::item("Paste Item"), Act::PasteInto(folder));
    }
    menu.separator();
    let trashed = Path::new(line.key.as_ref())
        .parent()
        .is_some_and(|dir| is_trash(&dir.to_string_lossy()));
    if trashed {
        menu.item(MenuEntry::item("Put Back"), Act::Trash);
        menu.item(MenuEntry::item("Delete Immediately…"), Act::DeleteNow);
        menu.item(MenuEntry::item("Empty Trash"), Act::EmptyTrash);
    } else {
        menu.item(MenuEntry::item("Move to Trash"), Act::Trash);
    }
    menu.separator();
    menu.item(MenuEntry::item("Get Info"), Act::GetInfo);
    menu.item(
        MenuEntry::item("Show Inspector").option_alternate(),
        Act::Inspector,
    );
    if many {
        menu.item(
            alternate("Get Summary Info", Modifiers::control()),
            Act::SummaryInfo,
        );
    }
    menu.item(
        MenuEntry::item(if many { "Rename…" } else { "Rename" }),
        Act::Rename,
    );
    let compress = subject("Compress", &line.row.display, many, chosen);
    let plain = match many {
        true => "Compress".to_string(),
        false => compress.clone(),
    };
    menu.item(MenuEntry::item(plain), Act::Compress);
    menu.item(MenuEntry::item(compress).option_alternate(), Act::Compress);
    menu.item(MenuEntry::item("Duplicate"), Act::Duplicate);
    let exactly = Modifiers {
        alt: true,
        shift: true,
        ..Modifiers::default()
    };
    menu.item(alternate("Duplicate Exactly", exactly), Act::Duplicate);
    menu.item(MenuEntry::item("Make Alias"), Act::MakeAlias);
    menu.item(MenuEntry::item("Quick Look"), Act::QuickLook);
    menu.item(
        MenuEntry::item(subject("Slideshow", &line.row.display, many, chosen)).option_alternate(),
        Act::Slideshow,
    );
    menu.item(MenuEntry::item("Copy"), Act::Copy);
    let pathname = if many {
        format!("Copy {chosen} Items as Pathnames")
    } else {
        format!("Copy “{}” as Pathname", line.row.display)
    };
    menu.item(
        MenuEntry::item(pathname).option_alternate(),
        Act::CopyPathnames,
    );
    menu.separator();
    menu.item(MenuEntry::item("Share…"), Act::Share);
    menu.separator();
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

fn open_with(menu: &mut Menu, line: &Line) {
    let apps = crate::openwith::apps_for(Path::new(line.key.as_ref()));
    if apps.is_empty() {
        menu.item(MenuEntry::item("Open With").disabled(), Act::Nothing);
        menu.item(
            MenuEntry::item("Always Open With")
                .disabled()
                .option_alternate(),
            Act::Nothing,
        );
        return;
    }
    let rows = |always: bool| {
        apps.iter()
            .map(|app| {
                let title = match app.default {
                    true => format!("{} (default)", app.name),
                    false => app.name.clone(),
                };
                let at = intern(&app.path.to_string_lossy());
                let act = match always {
                    true => Act::AlwaysOpenWith(at),
                    false => Act::OpenWith(at),
                };
                (MenuEntry::item(title), act)
            })
            .collect()
    };
    menu.submenu("Open With", rows(false));
    menu.alternate_submenu("Always Open With", rows(true));
}

/// The folder a row opens, following a Finder alias to its original.
fn folder_behind(line: &Line) -> Option<&'static str> {
    if line.row.opens.is_some() || line.row.uti != "com.apple.alias-file" {
        return line.row.opens;
    }
    let original = original_of(Path::new(line.key.as_ref()))?;
    let package = original
        .file_name()
        .is_some_and(|name| is_package_name(&name.to_string_lossy()));
    (original.is_dir() && !package).then(|| intern_path(&original))
}

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

pub(crate) fn sidebar(id: &SharedString) -> Menu {
    match id.strip_prefix("tag:") {
        Some(name) => tag_row(id, name),
        None if id.starts_with('/') => place_row(id),
        None => Menu::default(),
    }
}

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
    menu.item(
        MenuEntry::item("Open in New Window").option_alternate(),
        Act::OpenWindowAt(at),
    );
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
    menu.item(
        MenuEntry::item("Show Inspector").option_alternate(),
        Act::InspectorAt(at),
    );
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
        menu.item(MenuEntry::item("Add to Dock"), Act::AddToDock(at));
    }
    menu
}

fn holds(path: &Path) -> bool {
    !(volumes::standard(volumes::Standard::Trash).is_some_and(|trash| trash == path)
        || crate::home().join("Library/Mobile Documents/com~apple~CloudDocs") == path
        || path == Path::new("/Network"))
}

pub(crate) fn header(columns: &[ColumnSpec]) -> Menu {
    let mut menu = Menu::default();
    for (title, column) in COLUMNS {
        match column {
            Some(column) => {
                let shown = columns.iter().any(|spec| spec.column == column);
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

/// Stands in for the row above it while exactly `modifiers` are held.
fn alternate(title: &str, modifiers: Modifiers) -> MenuEntry {
    MenuEntry::item(title).key("", modifiers).option_alternate()
}

fn view_rows(demo: &FinderDemo) -> Vec<(MenuEntry, Act)> {
    VIEWS
        .iter()
        .map(|&(title, view)| {
            (
                MenuEntry::item(title).checked(demo.view == view),
                Act::View(view),
            )
        })
        .collect()
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
