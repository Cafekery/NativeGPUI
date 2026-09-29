use std::cell::Cell;
use std::collections::HashSet;
use std::path::Path;

use finder_fs::kind::is_package_name;
use finder_fs::volumes;
use gpui::{Hsla, Modifiers, SharedString};
use native_gpui::{MenuEntry, MenuView};

use super::actions::original_of;
use super::{Act, Menu};
use crate::app::FinderDemo;
use crate::data::{Column, Line, SORT_KEYS, SORT_TITLES, intern, intern_path, is_trash};
use crate::ops::clipboard::{Pasting, can_paste};
use crate::tags::{finder_color, finder_color_rgb, tags};
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};
use crate::views::header::{ColumnSpec, column_title};
use crate::views::icon::Arrangement;

thread_local! {
    static SWATCH: Cell<Option<Option<usize>>> = const { Cell::new(None) };
}

const COLUMNS: [(&str, Option<Column>); 11] = [
    ("Date Modified", Some(Column::Modified)),
    ("Date Created", Some(Column::Created)),
    ("Date Last Opened", Some(Column::Opened)),
    ("Date Added", Some(Column::Added)),
    ("Size", Some(Column::Size)),
    ("Kind", Some(Column::Kind)),
    ("Last Modified By", None),
    ("Shared By", None),
    ("Version", Some(Column::Version)),
    ("Comments", Some(Column::Comments)),
    ("Tags", Some(Column::Tags)),
];

const CLEAN_UP_BY: [Column; 6] = [
    Column::Name,
    Column::Kind,
    Column::Modified,
    Column::Created,
    Column::Size,
    Column::Tags,
];

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
        paste_rows(&mut menu, demo.target());
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
    match demo.view == ICON_VIEW {
        true => icon_rows(&mut menu, demo),
        false => menu.submenu("Sort By", sort_rows(demo)),
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
            let refused = match many {
                true => selected
                    .iter()
                    .filter_map(|key| crate::data::path_line(intern(key)))
                    .all(|line| refuses(&line)),
                false => refuses(line),
            };
            let open = |title| match refused {
                true => MenuEntry::item(title).disabled(),
                false => MenuEntry::item(title),
            };
            menu.item(open(plural("Open in New Tab", many)), tab);
            menu.item(
                open(plural("Open in New Window", many)).option_alternate(),
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
        paste_rows(&mut menu, folder);
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
    menu.item(exactly("Duplicate Exactly"), Act::Duplicate);
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
    let carried: Vec<Vec<usize>> = match selected.contains(&line.key) {
        true => selected
            .iter()
            .map(|key| crate::tags::row_tags(key))
            .collect(),
        false => vec![crate::tags::row_tags(&line.key)],
    };
    let button = match !many && line.row.opens.is_some() {
        true => "Customize Folder…",
        false => "Tags…",
    };
    menu.view(
        MenuView::swatches_with_button(
            tag_swatches(&carried),
            true,
            ("tag", button, true),
            |hit| SWATCH.set(Some(hit)),
        ),
        Act::Swatch,
    );
    menu.separator();
    let options = match crate::preview::options::is_open() {
        true => "Hide Preview Options",
        false => "Show Preview Options",
    };
    menu.item(MenuEntry::item(options), Act::PreviewOptions);
    menu.separator();
    let faces: Vec<_> = match selected.contains(&line.key) {
        true => demo_faces(selected),
        false => vec![line.row.face],
    };
    menu.submenu(
        "Quick Actions",
        crate::quick_actions::menu_rows(&crate::quick_actions::offered(&faces)),
    );
    menu
}

pub(super) fn tag_swatches(carried: &[Vec<usize>]) -> impl Iterator<Item = (Hsla, String)> + '_ {
    tags().iter().enumerate().map(|(index, tag)| {
        let removes = !carried.is_empty() && carried.iter().all(|tags| tags.contains(&index));
        let verb = if removes { "Remove" } else { "Add" };
        (
            finder_color(tag.color),
            format!("{verb} \u{201c}{}\u{201d} label", tag.name),
        )
    })
}

fn paste_rows(menu: &mut Menu, into: &'static str) {
    menu.item(
        MenuEntry::item("Paste Item"),
        Act::Paste(into, Pasting::Copy),
    );
    menu.item(
        exactly("Paste Item Exactly"),
        Act::Paste(into, Pasting::Exactly),
    );
}

fn demo_faces(selected: &HashSet<SharedString>) -> Vec<crate::data::Face> {
    selected
        .iter()
        .filter_map(|key| crate::data::path_line(crate::data::intern(key)))
        .map(|line| line.row.face)
        .collect()
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

fn refuses(line: &Line) -> bool {
    line.row.opens.is_some() && !line.row.readable
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
    sorts.extend(crate::data::SORT_KEYS.iter().map(|column| {
        (
            MenuEntry::item(column_title(*column)).checked(arranged == Arrangement::By(*column)),
            Act::Arrange(Arrangement::By(*column)),
        )
    }));
    menu.submenu("Sort By", sorts);
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

/// Stands in for the row above it while exactly `modifiers` are held.
fn alternate(title: &str, modifiers: Modifiers) -> MenuEntry {
    MenuEntry::item(title).key("", modifiers).option_alternate()
}

fn exactly(title: &str) -> MenuEntry {
    let modifiers = Modifiers {
        alt: true,
        shift: true,
        ..Modifiers::default()
    };
    alternate(title, modifiers)
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
    SORT_KEYS
        .into_iter()
        .zip(SORT_TITLES)
        .map(|(column, title)| {
            (
                MenuEntry::item(title).checked(demo.sort == column),
                Act::Sort(column),
            )
        })
        .collect()
}

/// The dot hit in an item's tag row, or `Some(None)` for its button.
pub(super) fn picked_swatch() -> Option<Option<usize>> {
    SWATCH.with(Cell::take)
}
