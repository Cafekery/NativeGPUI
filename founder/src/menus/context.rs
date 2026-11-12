use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use finder_fs::kind::is_package_name;
use finder_fs::volumes;
use gpui::{Hsla, Modifiers, SharedString};
use native_gpui::{MenuEntry, MenuView};

use super::actions::original_of;
use super::{Act, Menu};
use crate::app::FinderDemo;
use crate::data::{Column, Line, SORT_KEYS, SORT_TITLES, intern, intern_path, is_trash};
use crate::ops::clipboard::{Pasting, can_paste};
use crate::tags::{RowTag, finder_color, finder_color_rgb, tags};
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
    ("Shared By", Some(Column::SharedBy)),
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
    let groups = MenuEntry::item("Use Groups").checked(demo.group().is_some());
    menu.item(
        if demo.view == GALLERY_VIEW {
            groups.disabled()
        } else {
            groups
        },
        Act::Groups,
    );
    if demo.view == ICON_VIEW {
        icon_rows(&mut menu, demo)
    } else {
        menu.submenu("Sort By", sort_rows(demo))
    }
    menu.item(MenuEntry::item("Show View Options"), Act::ViewOptions);
    menu
}

pub(crate) fn item(selected: &HashSet<SharedString>, line: &Line) -> Menu {
    let chosen = selected.len();
    let in_selection = selected.contains(&line.key);
    let many = chosen > 1 && in_selection;
    if crate::remote::is_remote(&line.key) {
        return remote_item(line, many, chosen);
    }
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
            let (tab, window) = if many {
                (Act::OpenTabs, Act::OpenWindow)
            } else {
                (Act::OpenTab(folder), Act::OpenWindowAt(folder))
            };
            let refused = if many {
                selected_lines(selected).all(|line| unlistable_folder(&line))
            } else {
                unlistable_folder(line)
            };
            let open = |title| {
                if refused {
                    MenuEntry::item(title).disabled()
                } else {
                    MenuEntry::item(title)
                }
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
        None => menu.item(MenuEntry::item("Open"), Act::Open),
    }
    let package = if line.row.package {
        Some(intern(&line.key))
    } else {
        ((original && !many).then(|| original_of(Path::new(line.key.as_ref()))))
            .flatten()
            .filter(|original| original.is_dir() && is_package(original))
            .map(|original| intern_path(&original))
    };
    if let Some(package) = package.filter(|_| !many) {
        menu.item(
            MenuEntry::item("Show Package Contents"),
            Act::ShowPackage(package),
        );
    }
    if folder.is_none() && !original {
        open_with(&mut menu, line);
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
        let items: Vec<PathBuf> = if many {
            selected
                .iter()
                .map(|key| PathBuf::from(key.as_ref()))
                .collect()
        } else {
            vec![PathBuf::from(line.key.as_ref())]
        };
        let put_back = MenuEntry::item("Put Back");
        if crate::ops::put_back::origins(&items)
            .iter()
            .any(Option::is_some)
        {
            menu.item(put_back, Act::Trash);
        } else {
            menu.item(put_back.disabled(), Act::Nothing);
        }
        menu.item(MenuEntry::item("Delete Immediately…"), Act::DeleteNow);
        menu.item(MenuEntry::item("Empty Trash"), Act::EmptyTrash);
    } else {
        menu.item(MenuEntry::item("Move to Trash").symbol("trash"), Act::Trash);
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
    let unlistable = !many && unlistable_folder(line);
    if !unlistable {
        let compress = subject("Compress", &line.row.display, many, chosen);
        let plain = if many {
            "Compress".to_string()
        } else {
            compress.clone()
        };
        menu.item(MenuEntry::item(plain), Act::Compress);
        menu.item(MenuEntry::item(compress).option_alternate(), Act::Compress);
    }
    menu.item(MenuEntry::item("Duplicate"), Act::Duplicate);
    menu.item(exactly("Duplicate Exactly"), Act::Duplicate);
    menu.item(MenuEntry::item("Move to…"), Act::MoveTo);
    menu.item(MenuEntry::item("Make Alias"), Act::MakeAlias);
    let look = if unlistable {
        subject("Quick Look", &line.row.display, false, chosen)
    } else {
        "Quick Look".to_string()
    };
    menu.item(MenuEntry::item(look), Act::QuickLook);
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
    if !unlistable {
        menu.item(
            MenuEntry::item("Share…").symbol("square.and.arrow.up"),
            Act::Share,
        );
        menu.separator();
    }
    let carried: Vec<Vec<RowTag>> = if in_selection {
        selected
            .iter()
            .map(|key| crate::tags::row_tags(key))
            .collect()
    } else {
        vec![crate::tags::row_tags(&line.key)]
    };
    menu.view(
        swatch_row(&carried, !many && line.row.opens.is_some()),
        Act::Swatch,
    );
    menu.separator();
    let options = if crate::preview::options::is_open() {
        "Hide Preview Options"
    } else {
        "Show Preview Options"
    };
    menu.item(MenuEntry::item(options), Act::PreviewOptions);
    menu.separator();
    let faces: Vec<_> = if in_selection {
        selected_lines(selected).map(|line| line.row.face).collect()
    } else {
        vec![line.row.face]
    };
    menu.nest(
        MenuEntry::item("Quick Actions").symbol("gearshape"),
        crate::quick_actions::menu_rows(&crate::quick_actions::offered(&faces)),
    );
    menu
}

fn remote_item(line: &Line, many: bool, chosen: usize) -> Menu {
    let mut menu = Menu::default();
    let root = crate::remote::is_root(&line.key);
    if many && !root {
        menu.item(
            MenuEntry::item(format!("New Folder with Selection ({chosen} Items)")),
            Act::NewFolderWith,
        );
    }
    match line.row.opens {
        Some(folder) => {
            let (tab, window) = if many {
                (Act::OpenTabs, Act::OpenWindow)
            } else {
                (Act::OpenTab(folder), Act::OpenWindowAt(folder))
            };
            menu.item(MenuEntry::item(plural("Open in New Tab", many)), tab);
            menu.item(
                MenuEntry::item(plural("Open in New Window", many)).option_alternate(),
                window,
            );
        }
        None => menu.item(MenuEntry::item("Open"), Act::Open),
    }
    if line.row.package && !many {
        let package = Act::ShowPackage(intern(&line.key));
        menu.item(MenuEntry::item("Show Package Contents"), package);
    }
    if let Some(folder) = line.row.opens.filter(|_| can_paste()) {
        paste_rows(&mut menu, folder);
    }
    menu.separator();
    if !root {
        menu.item(MenuEntry::item("Move to Trash"), Act::Trash);
        menu.separator();
    }
    menu.item(MenuEntry::item("Get Info"), Act::GetInfo);
    menu.item(
        MenuEntry::item("Show Inspector").option_alternate(),
        Act::Inspector,
    );
    if root {
        return menu;
    }
    menu.item(
        MenuEntry::item(if many { "Rename…" } else { "Rename" }),
        Act::Rename,
    );
    menu.item(MenuEntry::item("Duplicate"), Act::Duplicate);
    menu.item(MenuEntry::item("Move to…"), Act::MoveTo);
    if many || line.row.opens.is_none() {
        menu.item(MenuEntry::item("Quick Look"), Act::QuickLook);
    }
    menu.item(MenuEntry::item("Copy"), Act::Copy);
    menu
}

pub(super) fn swatch_row(carried: &[Vec<RowTag>], lone_folder: bool) -> MenuView {
    let picked = !carried.is_empty();
    let button = if lone_folder {
        "Customize Folder…"
    } else {
        "Tags…"
    };
    MenuView::swatches_with_button(
        tag_swatches(carried),
        picked,
        ("tag", button, picked),
        |hit| SWATCH.set(Some(hit)),
    )
}

fn tag_swatches(carried: &[Vec<RowTag>]) -> Vec<(Hsla, String)> {
    tags()
        .iter()
        .map(|tag| {
            let removes = !carried.is_empty()
                && carried
                    .iter()
                    .all(|held| held.iter().any(|it| *it.name == *tag.name));
            let verb = if removes { "Remove" } else { "Add" };
            (
                finder_color(tag.color),
                format!("{verb} \u{201c}{}\u{201d} label", tag.name),
            )
        })
        .collect()
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

fn selected_lines(selected: &HashSet<SharedString>) -> impl Iterator<Item = Line> + '_ {
    selected
        .iter()
        .filter_map(|key| crate::data::path_line(intern(key)))
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
                let title = if app.default {
                    format!("{} (default)", app.name)
                } else {
                    app.name.clone()
                };
                let at = intern(&app.path.to_string_lossy());
                let act = if always {
                    Act::AlwaysOpenWith(at)
                } else {
                    Act::OpenWith(at)
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
    (original.is_dir() && !is_package(&original)).then(|| intern_path(&original))
}

fn is_package(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| is_package_name(&name.to_string_lossy()))
}

pub(super) fn unlistable_folder(line: &Line) -> bool {
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
    if !crate::remote::is_remote(id) {
        menu.item(MenuEntry::item("Open in Terminal"), Act::OpenTerminal(id));
    }
    menu
}

pub(crate) fn sidebar(id: &SharedString) -> Menu {
    match id.strip_prefix("tag:") {
        Some(name) => tag_row(id, name),
        None if id.starts_with('/') => place_row(id),
        None if crate::remote::is_remote(id) => server_row(intern(id)),
        None => Menu::default(),
    }
}

fn server_row(at: &'static str) -> Menu {
    let mut menu = Menu::default();
    menu.item(MenuEntry::item("Open in New Tab"), Act::OpenTab(at));
    menu.item(
        MenuEntry::item("Open in New Window").option_alternate(),
        Act::OpenWindowAt(at),
    );
    menu.separator();
    let host = crate::remote::host(at);
    menu.item(
        MenuEntry::item(format!("Eject \u{201c}{host}\u{201d}")),
        Act::Eject(at),
    );
    menu.separator();
    menu.item(MenuEntry::item("Get Info"), Act::GetInfoAt(at));
    menu
}

fn tag_row(id: &SharedString, name: &str) -> Menu {
    let mut menu = Menu::default();
    let at = intern(id);
    menu.item(MenuEntry::item("Open in New Tab"), Act::OpenTab(at));
    menu.item(
        MenuEntry::item("Open in New Window").option_alternate(),
        Act::OpenWindowAt(at),
    );
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
    let name = intern(name);
    menu.item(
        MenuEntry::item("No Color").symbol("circle"),
        Act::TagColor(name, 0),
    );
    for tag in finder_fs::tags::standard() {
        let entry = MenuEntry::item(tag.name);
        menu.item(
            match finder_color_rgb(tag.color) {
                Some(color) => entry.dot(color),
                None => entry,
            },
            Act::TagColor(name, tag.color),
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

pub(crate) fn header(columns: &[ColumnSpec], here: &str) -> Menu {
    let mut menu = Menu::default();
    for (title, column) in COLUMNS {
        match column.filter(|column| column.offered_at(here)) {
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
    if many {
        format!("{title}s")
    } else {
        title.to_string()
    }
}

fn subject(verb: &str, name: &SharedString, many: bool, chosen: usize) -> String {
    if many {
        format!("{verb} {chosen} Items")
    } else {
        format!("{verb} “{name}”")
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
    sorts.extend(SORT_KEYS.into_iter().map(|column| {
        (
            MenuEntry::item(column_title(column)).checked(arranged == Arrangement::By(column)),
            Act::Arrange(Arrangement::By(column)),
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
            .into_iter()
            .map(|column| {
                (
                    MenuEntry::item(column_title(column)),
                    Act::CleanUpBy(column),
                )
            })
            .collect(),
    );
}

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

pub(super) fn picked_swatch() -> Option<Option<usize>> {
    SWATCH.with(Cell::take)
}
