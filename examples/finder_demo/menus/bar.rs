use std::cell::Cell;
use std::path::Path;

use finder_fs::volumes::{self, Standard};
use gpui::{
    Action, AnyWindowHandle, App, Context, Div, InteractiveElement, SharedString, Window, actions,
};
use native_gpui::{
    MenuCommand, MenuSection, add_menu, extend_menu, on_menu_enabled, on_menu_state, on_menu_title,
    prepend_menu, replace_edit_rows, set_toolbar_search_active,
};

use super::edit::PasteExactly;
use crate::app::FinderDemo;
use crate::chrome::criteria_bar::Criterion;
use crate::chrome::toolbar::SEARCH_ID;
use crate::data::{Column, intern_path};
use crate::menus::Act;
use crate::ops::clipboard::Pasting;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};

actions!(
    finder_demo,
    [
        Back,
        CleanUp,
        CleanUpByName,
        CleanUpByKind,
        CleanUpByModified,
        CleanUpByCreated,
        CleanUpBySize,
        CleanUpByTags,
        UseGroups,
        ShowViewOptions,
        Cut,
        Paste,
        MoveHere,
        CopyNames,
        CopyPath,
        DeselectAll,
        ShowClipboard,
        Compress,
        Duplicate,
        DuplicateExactly,
        EjectSelection,
        EmptyTrash,
        FindHere,
        FindByName,
        Forward,
        GetInfo,
        GetSummaryInfo,
        AddToSidebar,
        NewSmartFolder,
        GoAirDrop,
        GoApplications,
        GoComputer,
        GoDesktop,
        GoDocuments,
        GoDownloads,
        GoHome,
        GoICloud,
        GoLibrary,
        GoNetwork,
        GoRecents,
        GoShared,
        GoToFolder,
        ConnectToServer,
        GoUtilities,
        GoRecent0,
        GoRecent1,
        GoRecent2,
        GoRecent3,
        GoRecent4,
        ClearRecents,
        NewFolder,
        NewFolderWithSelection,
        NewTextFile,
        NewTab,
        NewWindow,
        MakeAlias,
        OpenSelection,
        QuickLook,
        Slideshow,
        Print,
        Share,
        OpenInNewTab,
        OpenInNewWindow,
        ShowOriginal,
        RenameSelection,
        SelectAll,
        ShowInspector,
        SortByAdded,
        SortByCreated,
        SortByKind,
        SortByModified,
        SortByName,
        SortByOpened,
        SortBySize,
        SortByTags,
        ToggleHidden,
        TogglePathBar,
        TogglePreview,
        ToggleSidebar,
        ToggleStatusBar,
        Trash,
        DeleteImmediately,
        Up,
        UpInNewWindow,
        SelectStartupDisk,
        ShowSettings,
        CycleWindows,
        ShowProgressWindow,
        ViewColumns,
        ViewGallery,
        ViewIcons,
        ViewList,
        ZoomIn,
        ZoomOut,
    ]
);

thread_local! {
    /// Menu validation cannot read the window, so check marks and enabling follow this mirror.
    static STATE: Cell<(usize, Column, bool, bool, bool, bool)> =
        const { Cell::new((LIST_VIEW, Column::Name, false, false, false, false)) };
    static CHOSEN: Cell<((u64, u64, usize), bool, bool, bool)> =
        const { Cell::new(((u64::MAX, 0, 0), false, false, false)) };
}

/// Whether the chosen items refuse to open, can be shared, and all sit in iCloud Drive.
pub(crate) fn chosen_state(demo: &FinderDemo) -> (bool, bool, bool) {
    let seen = (demo.selected.rev(), demo.folders.changed(), demo.view);
    let (at, refused, shareable, cloud) = CHOSEN.get();
    if at == seen {
        return (refused, shareable, cloud);
    }
    let chosen = demo.chosen_lines();
    // Finder keeps the Open rows off while every chosen item is a folder the user can't list.
    let refused = !chosen.is_empty()
        && chosen
            .iter()
            .all(|line| line.row.opens.is_some() && !line.row.readable);
    // Finder won't share a folder the user can't list, even beside readable items.
    let shareable = chosen
        .iter()
        .all(|line| line.row.opens.is_none() || line.row.readable);
    let cloud = finder_fs::icloud::drive().is_some_and(|drive| {
        shareable
            && !chosen.is_empty()
            && chosen
                .iter()
                .all(|line| Path::new(line.key.as_ref()).starts_with(&drive))
    });
    CHOSEN.set((seen, refused, shareable, cloud));
    (refused, shareable, cloud)
}

pub(crate) fn remember(demo: &FinderDemo) {
    super::edit::remember(demo);
    STATE.set((
        demo.view,
        demo.sort,
        demo.grouped,
        !demo.selected.is_empty(),
        chosen_state(demo).0,
        crate::data::is_trash(demo.here()),
    ));
}

fn enabled(action: &dyn Action) -> bool {
    if let Some(on) = super::edit::enabled(action) {
        return on;
    }
    let (_, _, _, chosen, refused, _) = STATE.get();
    let opens: [&dyn Action; 3] = [&OpenSelection, &OpenInNewTab, &OpenInNewWindow];
    if opens.iter().any(|row| action.partial_eq(*row)) {
        return !refused;
    }
    let rows: [&dyn Action; 13] = [
        &NewFolderWithSelection,
        &RenameSelection,
        &Compress,
        &Duplicate,
        &DuplicateExactly,
        &MakeAlias,
        &Share,
        &ShowOriginal,
        &Trash,
        &DeleteImmediately,
        &EjectSelection,
        &Print,
        &DeselectAll,
    ];
    chosen || !rows.iter().any(|row| action.partial_eq(*row))
}

fn checked(action: &dyn Action) -> Option<bool> {
    let (view, sort, grouped, _, _, _) = STATE.get();
    let rows: [(&dyn Action, bool); 13] = [
        (&ViewIcons, view == ICON_VIEW),
        (&ViewList, view == LIST_VIEW),
        (&ViewColumns, view == COLUMN_VIEW),
        (&ViewGallery, view == GALLERY_VIEW),
        (&SortByName, sort == Column::Name),
        (&SortByKind, sort == Column::Kind),
        (&SortByOpened, sort == Column::Opened),
        (&SortByAdded, sort == Column::Added),
        (&SortByModified, sort == Column::Modified),
        (&SortByCreated, sort == Column::Created),
        (&SortBySize, sort == Column::Size),
        (&SortByTags, sort == Column::Tags),
        (&UseGroups, grouped),
    ];
    rows.into_iter()
        .find(|(row, _)| action.partial_eq(*row))
        .map(|(_, on)| on)
}

fn titled(action: &dyn Action) -> Option<SharedString> {
    if let Some(title) = super::edit::titled(action) {
        return Some(title);
    }
    if action.partial_eq(&ShowInspector) {
        return Some(match crate::inspector::is_open() {
            true => "Hide Inspector".into(),
            false => "Show Inspector".into(),
        });
    }
    let in_trash = STATE.get().5;
    action.partial_eq(&Trash).then(|| {
        if in_trash {
            "Put Back"
        } else {
            "Move to Trash"
        }
        .into()
    })
}

fn cycle_windows(_: &CycleWindows, cx: &mut App) {
    let windows: Vec<AnyWindowHandle> = cx
        .windows()
        .into_iter()
        .filter(|window| window.downcast::<FinderDemo>().is_some())
        .collect();
    let Some(&first) = windows.first() else {
        return;
    };
    let at = cx
        .active_window()
        .and_then(|here| windows.iter().position(|window| *window == here));
    let next = at.map_or(first, |at| windows[(at + 1) % windows.len()]);
    next.update(cx, |_, window, _| window.activate_window())
        .ok();
}

pub(crate) fn install(cx: &mut App) {
    cx.on_action(cycle_windows);
    cx.on_action(crate::clipboard_window::show);
    cx.on_action(crate::progress_window::show);
    cx.on_action(|_: &ShowSettings, cx| crate::settings::open(cx));
    on_menu_state(checked);
    on_menu_enabled(enabled);
    on_menu_title(titled);
    prepend_menu(
        MenuSection::File,
        [
            MenuCommand::new("New Finder Window", NewWindow).key("cmd-n"),
            MenuCommand::new("New Folder", NewFolder).key("cmd-shift-n"),
            MenuCommand::new("New Text File", NewTextFile),
            MenuCommand::new("New Folder with Selection", NewFolderWithSelection).key("cmd-ctrl-n"),
            MenuCommand::new("New Smart Folder", NewSmartFolder),
            MenuCommand::separator(),
            MenuCommand::new("Open", OpenSelection).key("cmd-o"),
            MenuCommand::new("Open in New Tab", OpenInNewTab).key("cmd-ctrl-o"),
            MenuCommand::new("Open in New Window", OpenInNewWindow).key("cmd-alt-o"),
            MenuCommand::separator(),
            MenuCommand::new("Get Info", GetInfo).key("cmd-i"),
            MenuCommand::new("Show Inspector", ShowInspector).key("cmd-alt-i"),
            MenuCommand::new("Get Summary Info", GetSummaryInfo)
                .key("cmd-ctrl-i")
                .option_alternate(),
            MenuCommand::new("Rename", RenameSelection),
            MenuCommand::new("Compress", Compress),
            MenuCommand::new("Duplicate", Duplicate).key("cmd-d"),
            MenuCommand::new("Duplicate Exactly", DuplicateExactly)
                .key("cmd-alt-shift-d")
                .option_alternate(),
            MenuCommand::new("Make Alias", MakeAlias).key("cmd-ctrl-a"),
            MenuCommand::new("Quick Look", QuickLook).key("cmd-y"),
            MenuCommand::new("Slideshow", Slideshow)
                .key("cmd-alt-y")
                .option_alternate(),
            MenuCommand::separator(),
            MenuCommand::new("Share…", Share),
            MenuCommand::separator(),
            MenuCommand::new("Show Original", ShowOriginal).key("cmd-r"),
            MenuCommand::new("Add to Sidebar", AddToSidebar).key("cmd-ctrl-t"),
            MenuCommand::separator(),
            MenuCommand::new("Move to Trash", Trash).key("cmd-backspace"),
            MenuCommand::new("Delete Immediately…", DeleteImmediately)
                .key("cmd-alt-backspace")
                .option_alternate(),
            MenuCommand::new("Eject", EjectSelection).key("cmd-e"),
            MenuCommand::separator(),
            MenuCommand::new("Print", Print).key("cmd-p"),
            MenuCommand::separator(),
        ],
    );
    extend_menu(
        MenuSection::App,
        [MenuCommand::new("Empty Trash…", EmptyTrash).key("cmd-shift-backspace")],
    );
    extend_menu(
        MenuSection::App,
        [
            MenuCommand::new("Settings…", ShowSettings).key("cmd-,"),
            MenuCommand::separator(),
        ],
    );
    extend_menu(
        MenuSection::Window,
        [
            MenuCommand::new("Cycle Through Windows", CycleWindows).key("cmd-`"),
            MenuCommand::new("Show Progress Window", ShowProgressWindow),
        ],
    );
    replace_edit_rows(super::edit::rows());
    prepend_menu(
        MenuSection::View,
        [
            MenuCommand::new("as Icons", ViewIcons).key("cmd-1"),
            MenuCommand::new("as List", ViewList).key("cmd-2"),
            MenuCommand::new("as Columns", ViewColumns).key("cmd-3"),
            MenuCommand::new("as Gallery", ViewGallery).key("cmd-4"),
            MenuCommand::separator(),
            MenuCommand::submenu(
                "Sort By",
                [
                    MenuCommand::new("Name", SortByName).key("cmd-ctrl-alt-1"),
                    MenuCommand::new("Kind", SortByKind).key("cmd-ctrl-alt-2"),
                    MenuCommand::new("Date Last Opened", SortByOpened).key("cmd-ctrl-alt-3"),
                    MenuCommand::new("Date Added", SortByAdded).key("cmd-ctrl-alt-4"),
                    MenuCommand::new("Date Modified", SortByModified).key("cmd-ctrl-alt-5"),
                    MenuCommand::new("Date Created", SortByCreated),
                    MenuCommand::new("Size", SortBySize).key("cmd-ctrl-alt-6"),
                    MenuCommand::new("Tags", SortByTags).key("cmd-ctrl-alt-7"),
                ],
            ),
            MenuCommand::new("Use Groups", UseGroups).key("cmd-ctrl-0"),
            MenuCommand::new("Show View Options", ShowViewOptions).key("cmd-j"),
            MenuCommand::new("Clean Up", CleanUp),
            MenuCommand::submenu(
                "Clean Up By",
                [
                    MenuCommand::new("Name", CleanUpByName).key("cmd-alt-1"),
                    MenuCommand::new("Kind", CleanUpByKind).key("cmd-alt-2"),
                    MenuCommand::new("Date Modified", CleanUpByModified).key("cmd-alt-5"),
                    MenuCommand::new("Date Created", CleanUpByCreated),
                    MenuCommand::new("Size", CleanUpBySize).key("cmd-alt-6"),
                    MenuCommand::new("Tags", CleanUpByTags).key("cmd-alt-7"),
                ],
            ),
            MenuCommand::separator(),
            MenuCommand::new("Show Preview", TogglePreview).key("cmd-shift-p"),
            MenuCommand::new("Show Path Bar", TogglePathBar).key("cmd-alt-p"),
            MenuCommand::new("Show Status Bar", ToggleStatusBar).key("cmd-/"),
            MenuCommand::new("Show Hidden Files", ToggleHidden).key("cmd-shift-."),
            MenuCommand::separator(),
        ],
    );
    add_menu("Go", go_rows(&[]));
}

pub(crate) fn go_rows(recents: &[(&'static str, String)]) -> Vec<MenuCommand> {
    let mut recent_rows: Vec<MenuCommand> = recents
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, (_, title))| match index {
            0 => MenuCommand::new(title.clone(), GoRecent0),
            1 => MenuCommand::new(title.clone(), GoRecent1),
            2 => MenuCommand::new(title.clone(), GoRecent2),
            3 => MenuCommand::new(title.clone(), GoRecent3),
            _ => MenuCommand::new(title.clone(), GoRecent4),
        })
        .collect();
    if !recent_rows.is_empty() {
        recent_rows.push(MenuCommand::separator());
    }
    recent_rows.push(MenuCommand::new("Clear Menu", ClearRecents));
    let mut rows = go_head();
    rows.push(MenuCommand::submenu("Recent Folders", recent_rows));
    rows
}

fn go_head() -> Vec<MenuCommand> {
    vec![
        MenuCommand::new("Back", Back).key("cmd-["),
        MenuCommand::new("Forward", Forward).key("cmd-]"),
        MenuCommand::new("Enclosing Folder", Up).key("cmd-up"),
        MenuCommand::new("Enclosing Folder in New Window", UpInNewWindow).key("cmd-ctrl-up"),
        MenuCommand::new("Select Startup Disk", SelectStartupDisk).key("cmd-shift-up"),
        MenuCommand::separator(),
        MenuCommand::new("Recents", GoRecents).key("cmd-shift-f"),
        MenuCommand::new("Documents", GoDocuments).key("cmd-shift-o"),
        MenuCommand::new("Desktop", GoDesktop).key("cmd-shift-d"),
        MenuCommand::new("Downloads", GoDownloads).key("cmd-alt-l"),
        MenuCommand::new("Home", GoHome).key("cmd-shift-h"),
        MenuCommand::new("Library", GoLibrary).option_only(),
        MenuCommand::new("Computer", GoComputer).key("cmd-shift-c"),
        MenuCommand::new("AirDrop", GoAirDrop).key("cmd-shift-r"),
        MenuCommand::new("Network", GoNetwork).key("cmd-shift-k"),
        MenuCommand::new("iCloud Drive", GoICloud).key("cmd-shift-i"),
        MenuCommand::new("Shared", GoShared).key("cmd-shift-s"),
        MenuCommand::new("Applications", GoApplications).key("cmd-shift-a"),
        MenuCommand::new("Utilities", GoUtilities).key("cmd-shift-u"),
        MenuCommand::separator(),
        MenuCommand::new("Go to Folder…", GoToFolder).key("cmd-shift-g"),
        MenuCommand::new("Connect to Server…", ConnectToServer).key("cmd-k"),
        MenuCommand::separator(),
    ]
}

impl FinderDemo {
    fn go_recent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.recents.get(index).map(|(id, _)| *id) {
            self.go(id, window, cx);
        }
    }

    fn go_place(&mut self, which: Standard, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = volumes::standard(which) {
            self.go_to(&path, window, cx);
        }
    }

    fn go_to(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.go(intern_path(path), window, cx);
        }
    }

    pub(crate) fn menu_bar_actions(&mut self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .on_action(cx.listener(|this, _: &NewWindow, _, cx| {
                this.open_new_window(cx);
            }))
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                this.open_tab(super::actions::new_window_target(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
                this.run_menu(Act::NewFolder, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewFolderWithSelection, _, cx| {
                this.new_folder_with_selection(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewTextFile, _, cx| {
                this.new_file(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                this.paste(Pasting::Copy, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveHere, _, cx| {
                this.paste(Pasting::Move, cx);
            }))
            .on_action(cx.listener(|this, _: &PasteExactly, _, cx| {
                this.paste(Pasting::Exactly, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSelection, window, cx| {
                this.run_menu(Act::Open, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenInNewTab, window, cx| {
                if let Some(id) = this.chosen_folders().first().copied() {
                    this.open_tab(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenInNewWindow, window, cx| {
                this.run_menu(Act::OpenWindow, window, cx);
            }))
            .on_action(cx.listener(|this, _: &MakeAlias, window, cx| {
                this.run_menu(Act::MakeAlias, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Compress, window, cx| {
                this.run_menu(Act::Compress, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowOriginal, window, cx| {
                this.show_original(window, cx);
            }))
            .on_action(cx.listener(|this, _: &AddToSidebar, _, cx| {
                this.add_to_sidebar(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &EjectSelection, window, cx| {
                this.eject_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &EmptyTrash, _, cx| {
                this.empty_trash(cx);
            }))
            .on_action(cx.listener(|this, _: &FindHere, window, cx| {
                set_toolbar_search_active(window, SEARCH_ID, true);
                this.search_editing = true;
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &NewSmartFolder, window, _| {
                set_toolbar_search_active(window, SEARCH_ID, true);
            }))
            .on_action(cx.listener(|this, _: &FindByName, window, cx| {
                this.criteria = vec![Criterion::name()];
                set_toolbar_search_active(window, SEARCH_ID, true);
                this.search_editing = true;
                this.run_search(cx);
            }))
            .on_action(cx.listener(|this, _: &GetInfo, window, cx| {
                this.run_menu(Act::GetInfo, window, cx);
            }))
            .on_action(cx.listener(|_, _: &GetSummaryInfo, _, cx| {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, None, true, cx));
            }))
            .on_action(cx.listener(|this, _: &ShowInspector, window, cx| {
                this.run_menu(Act::Inspector, window, cx);
            }))
            .on_action(cx.listener(|this, _: &RenameSelection, window, cx| {
                this.run_menu(Act::Rename, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Duplicate, window, cx| {
                this.run_menu(Act::Duplicate, window, cx);
            }))
            .on_action(cx.listener(|this, _: &DuplicateExactly, window, cx| {
                this.run_menu(Act::Duplicate, window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteImmediately, _, cx| {
                this.delete_selection(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Trash, window, cx| {
                this.run_menu(Act::Trash, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| {
                this.copy_pathnames(cx);
            }))
            .on_action(cx.listener(|this, _: &DeselectAll, _, cx| {
                this.selected.clear();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewIcons, window, cx| {
                this.pick_view(ICON_VIEW, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ViewList, window, cx| {
                this.pick_view(LIST_VIEW, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ViewColumns, window, cx| {
                this.pick_view(COLUMN_VIEW, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ViewGallery, window, cx| {
                this.pick_view(GALLERY_VIEW, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortByName, window, cx| {
                this.run_menu(Act::Sort(Column::Name), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortByKind, window, cx| {
                this.run_menu(Act::Sort(Column::Kind), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortByOpened, window, cx| {
                this.run_menu(Act::Sort(Column::Opened), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortByAdded, window, cx| {
                this.run_menu(Act::Sort(Column::Added), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortByModified, window, cx| {
                this.run_menu(Act::Sort(Column::Modified), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortByCreated, window, cx| {
                this.run_menu(Act::Sort(Column::Created), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortBySize, window, cx| {
                this.run_menu(Act::Sort(Column::Size), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortByTags, window, cx| {
                this.run_menu(Act::Sort(Column::Tags), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowViewOptions, window, cx| {
                this.run_menu(Act::ViewOptions, window, cx);
            }))
            .on_action(cx.listener(|this, _: &UseGroups, _, cx| {
                this.grouped = !this.grouped;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CleanUp, window, cx| {
                this.run_menu(Act::Sort(this.sort), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                if let Some(sidebar) = this.sidebar.as_ref() {
                    sidebar.toggle();
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TogglePreview, _, cx| {
                this.show_preview(!this.preview);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TogglePathBar, _, cx| {
                this.path_bar_shown = !this.path_bar_shown;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleStatusBar, window, cx| {
                this.toggle_status_bar(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleHidden, _, cx| {
                this.show_hidden = !this.show_hidden;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Up, window, cx| {
                this.up(window, cx);
            }))
            .on_action(cx.listener(|this, _: &UpInNewWindow, _, cx| {
                this.enclosing_in_new_window(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectStartupDisk, window, cx| {
                this.show_enclosing("/", window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoRecents, window, cx| {
                this.show_tag(crate::search::RECENTS, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoDocuments, window, cx| {
                this.go_place(Standard::Documents, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoDesktop, window, cx| {
                this.go_place(Standard::Desktop, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoDownloads, window, cx| {
                this.go_place(Standard::Downloads, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoHome, window, cx| {
                this.go_place(Standard::Home, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoLibrary, window, cx| {
                this.go_to(&crate::home().join("Library"), window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoApplications, window, cx| {
                this.go_place(Standard::Applications, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoComputer, window, cx| {
                this.go(crate::locations::COMPUTER, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoNetwork, window, cx| {
                this.go(crate::locations::NETWORK, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoShared, window, cx| {
                this.go(crate::locations::SHARED, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoUtilities, window, cx| {
                this.go_to(Path::new("/Applications/Utilities"), window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoAirDrop, window, cx| {
                this.go(crate::locations::AIRDROP, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoICloud, window, cx| {
                if let Some(drive) = finder_fs::icloud::drive() {
                    this.go_to(&drive, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &GoRecent0, window, cx| {
                this.go_recent(0, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoRecent1, window, cx| {
                this.go_recent(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoRecent2, window, cx| {
                this.go_recent(2, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoRecent3, window, cx| {
                this.go_recent(3, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoRecent4, window, cx| {
                this.go_recent(4, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ClearRecents, _, _| {
                this.recents.clear();
                native_gpui::set_menu("Go", go_rows(&this.recents));
            }))
            .on_action(cx.listener(|this, _: &GoToFolder, window, cx| {
                this.open_go(window, cx);
            }))
            .on_action(cx.listener(|_, _: &ConnectToServer, window, cx| {
                let demo = cx.entity().downgrade();
                crate::chrome::connect::show(demo, window.window_handle(), cx);
            }))
            .on_action(cx.listener(|this, _: &QuickLook, window, cx| {
                this.toggle_quick_look(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Slideshow, window, cx| {
                this.slideshow(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Print, _, _| {
                if let Err(error) = crate::openwith::print(&this.paths()) {
                    this.complain(&error);
                }
            }))
            .on_action(cx.listener(|this, _: &Share, window, cx| {
                this.share_selection(window, cx);
            }))
    }
}
