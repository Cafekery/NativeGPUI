use std::cell::Cell;
use std::collections::HashSet;
use std::path::Path;

use finder_fs::volumes;
use gpui::SharedString;
use native_gpui::{MenuEntry, MenuView};

use super::{Act, Menu};
use crate::app::FinderDemo;
use crate::data::{Column, Line, intern, is_trash};
use crate::ops::clipboard::can_paste;
use crate::tags::{finder_color, finder_color_rgb, tags};
use crate::view::ICON_VIEW;
use crate::views::header::{ColumnSpec, column_title};
use crate::views::icon::Arrangement;

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

/// Finder's Clean Up By offers no Date Last Opened or Date Added.
const CLEAN_UP_BY: [Column; 4] = [Column::Name, Column::Kind, Column::Modified, Column::Size];

pub(crate) fn background(demo: &FinderDemo) -> Menu {
    let mut menu = Menu::default();
    menu.item(MenuEntry::item("New Folder"), Act::NewFolder);
    menu.item(MenuEntry::item("New Text File"), Act::NewFile);
    menu.separator();
    if can_paste() {
        menu.item(MenuEntry::item("Paste Item"), Act::Paste);
        menu.separator();
    }
    menu.item(MenuEntry::item("Get Info"), Act::GetInfoAt(demo.target()));
    menu.item(
        MenuEntry::item("Show Inspector").option_alternate(),
        Act::Inspector,
    );
    menu.separator();
    menu.item(
        MenuEntry::item("Use Groups").checked(demo.grouped),
        Act::Groups,
    );
    match demo.view == ICON_VIEW {
        true => icon_rows(&mut menu, demo),
        false => menu.submenu("Sort By", sort_rows(demo)),
    }
    menu.item(MenuEntry::item("Show View Options"), Act::ViewOptions);
    menu.separator();
    menu.item(
        MenuEntry::item("Open in Terminal"),
        Act::OpenTerminal(demo.target()),
    );
    menu
}

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
            MenuEntry::item(plural("Open in New Window", many)).option_alternate(),
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
        MenuEntry::item(subject("Slideshow", &line.row.display, many, chosen)).option_alternate(),
        Act::Slideshow,
    );
    menu.item(MenuEntry::item(counted("Cut", many, chosen)), Act::Cut);
    menu.item(MenuEntry::item(counted("Copy", many, chosen)), Act::Copy);
    let pathname = if many {
        format!("Copy {chosen} Items as Pathnames")
    } else {
        format!("Copy “{}” as Pathname", line.row.display)
    };
    menu.item(
        MenuEntry::item(pathname).option_alternate(),
        Act::CopyPathnames,
    );
    if can_paste() {
        menu.item(MenuEntry::item("Paste Item"), Act::Paste);
        menu.item(
            MenuEntry::item("Move Item Here").option_alternate(),
            Act::MoveHere,
        );
    }
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

fn counted(verb: &str, many: bool, chosen: usize) -> String {
    match many {
        true => format!("{verb} {chosen} Items"),
        false => verb.to_string(),
    }
}

fn subject(verb: &str, name: &SharedString, many: bool, chosen: usize) -> String {
    match many {
        true => format!("{verb} {chosen} Items"),
        false => format!("{verb} “{name}”"),
    }
}

fn icon_rows(menu: &mut Menu, demo: &FinderDemo) {
    let Some(arranged) = demo.arrangement() else {
        menu.submenu("Sort By", sort_rows(demo));
        return;
    };
    let mut sorts = vec![
        (
            MenuEntry::item("None").checked(arranged == Arrangement::Free),
            Act::Arrange(Arrangement::Free),
        ),
        (MenuEntry::separator(), Act::Nothing),
        (
            MenuEntry::item("Snap to Grid").checked(arranged == Arrangement::Grid),
            Act::Arrange(Arrangement::Grid),
        ),
        (MenuEntry::separator(), Act::Nothing),
    ];
    sorts.extend(SORTABLE.iter().map(|column| {
        (
            MenuEntry::item(column_title(*column)).checked(arranged == Arrangement::By(*column)),
            Act::Arrange(Arrangement::By(*column)),
        )
    }));
    menu.submenu("Sort By", sorts);
    // Finder leaves both out while the icons are kept sorted.
    if !demo.cleans_up() {
        return;
    }
    menu.item(MenuEntry::item("Clean Up"), Act::CleanUp);
    menu.submenu(
        "Clean Up By",
        CLEAN_UP_BY
            .iter()
            .map(|column| {
                (
                    MenuEntry::item(column_title(*column)),
                    Act::CleanUpBy(*column),
                )
            })
            .collect(),
    );
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
