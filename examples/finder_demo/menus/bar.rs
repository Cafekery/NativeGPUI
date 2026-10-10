use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use block2::RcBlock;
use finder_fs::sidebar::{Special, listed_locations};
use finder_fs::volumes::{self, Standard};
use gpui::{
    Action, AnyWindowHandle, App, Context, Div, InteractiveElement, KeyBinding, SharedString,
    Window, actions,
};
use native_gpui::{
    MenuCommand, MenuSection, add_menu, extend_menu, on_menu_enabled, on_menu_state, on_menu_title,
    replace_about_row, replace_edit_rows, set_menu, set_toolbar_search_active,
};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSHelpManager, NSWorkspace};
use objc2_foundation::{
    NSBundle, NSError, NSFileManager, NSFileManagerUnmountOptions, NSRunLoop, NSString, NSURL,
    ns_string,
};

use super::actions::{open_first_window, open_windows};
use super::context::{swatch_row, unlistable_folder};
use super::edit::PasteExactly;
use crate::app::FinderDemo;
use crate::chrome::criteria_bar::Criterion;
use crate::chrome::toolbar::SEARCH_ID;
use crate::data::{Column, Line, intern_path, is_trash};
use crate::locations::{AIRDROP, COMPUTER, NETWORK, SHARED};
use crate::menus::Act;
use crate::ops::clipboard::Pasting;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};
use crate::views::icon::Arrangement;

actions!(
    finder_demo,
    [
        Back,
        CleanUp,
        CleanUpBy,
        CleanUpSelection,
        CleanUpByName,
        CleanUpByKind,
        CleanUpByModified,
        CleanUpByCreated,
        CleanUpBySize,
        CleanUpByTags,
        ArrangeNone,
        ArrangeGrid,
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
        CompressNamed,
        Duplicate,
        DuplicateExactly,
        EjectSelection,
        EjectOne,
        EjectAll,
        EmptyTrash,
        EmptyTrashNow,
        FindHere,
        FindByName,
        Forward,
        GetInfo,
        GetSummaryInfo,
        AddToSidebar,
        AddToDock,
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
        ClearRecents,
        NewFolder,
        NewFolderWithSelection,
        NewTab,
        NewWindow,
        MakeAlias,
        OpenSelection,
        QuickLook,
        Slideshow,
        Print,
        EditTags,
        Share,
        OpenInNewTab,
        OpenAndClose,
        OpenWithRow,
        AlwaysOpenWithRow,
        OpenWithStore,
        NoApps,
        ShowOriginal,
        RenameSelection,
        SelectAll,
        ShowInspector,
        SortByAdded,
        SortByCreated,
        ShowPreviewOptions,
        SortByKind,
        SortByModified,
        SortByName,
        SortByOpened,
        SortBySize,
        SortByTags,
        ToggleDefaultBrowser,
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
        ShowAbout,
        CycleWindows,
        ShowProgressWindow,
        ViewColumns,
        ViewGallery,
        ViewIcons,
        ViewList,
        ZoomIn,
        ZoomOut,
        MacUserGuide,
        TipsForYourMac,
        Acknowledgements,
    ]
);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = finder_demo, no_json, no_register)]
struct OpenWithApp {
    app: PathBuf,
    always: bool,
}

#[derive(Clone, PartialEq, Action)]
#[action(namespace = finder_demo, no_json, no_register)]
struct OpenWithOther {
    always: bool,
}

#[derive(Clone, PartialEq, Action)]
#[action(namespace = finder_demo, no_json, no_register)]
struct GoLocation(&'static str);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = finder_demo, no_json, no_register)]
struct GoRecent(usize);

#[derive(Clone, Copy)]
struct Mirror {
    view: usize,
    sort: Column,
    arranged: Option<Arrangement>,
    cleans_up: bool,
    grouped: bool,
    in_trash: bool,
    location: bool,
    info: bool,
    back: bool,
    forward: bool,
    busy: bool,
    preview: bool,
    path_bar: bool,
    status_bar: bool,
    options: bool,
}

#[derive(Default)]
pub(super) struct Chosen {
    seen: (u64, u64, usize, usize),
    pub(super) count: usize,
    folders: usize,
    refused: bool,
    shareable: bool,
    cloud: bool,
    original: bool,
    ejectable: bool,
    pub(super) name: SharedString,
    here: SharedString,
    pub(super) folder: SharedString,
    pub(super) on_disk: bool,
    files: Vec<PathBuf>,
    keys: Vec<SharedString>,
}

thread_local! {
    // Menu validation cannot read the window, so check marks, titles and enabling follow these.
    static MIRROR: Cell<Mirror> = const {
        Cell::new(Mirror {
            view: LIST_VIEW,
            sort: Column::Name,
            arranged: None,
            cleans_up: false,
            grouped: false,
            in_trash: false,
            location: false,
            info: false,
            back: false,
            forward: false,
            busy: false,
            preview: false,
            path_bar: false,
            status_bar: false,
            options: false,
        })
    };
    // The front window's; WORKED caches whichever window's toolbar asked last.
    static CHOSEN: RefCell<Rc<Chosen>> = RefCell::new(Rc::default());
    static WORKED: RefCell<Rc<Chosen>> = RefCell::new(Rc::new(Chosen {
        seen: (u64::MAX, 0, 0, 0),
        ..Chosen::default()
    }));
    static LOCATIONS: RefCell<Vec<(String, &'static str)>> = const { RefCell::new(Vec::new()) };
}

/// (refuse to open, can be shared, all in iCloud Drive) for the chosen items.
pub(crate) fn chosen_state(demo: &FinderDemo) -> (bool, bool, bool) {
    let chosen = chosen(demo);
    (chosen.refused, chosen.shareable, chosen.cloud)
}

fn chosen(demo: &FinderDemo) -> Rc<Chosen> {
    let here = demo.here();
    let seen = (
        demo.selected.rev(),
        demo.folders.changed(),
        demo.view,
        here.as_ptr() as usize,
    );
    if let Some(known) = WORKED.with_borrow(|worked| (worked.seen == seen).then(|| worked.clone()))
    {
        return known;
    }
    let lines = demo.chosen_lines();
    let refused = !lines.is_empty() && lines.iter().all(unlistable_folder);
    // Finder won't share a folder the user can't list, even beside readable items.
    let shareable = !lines.iter().any(unlistable_folder);
    let cloud = finder_fs::icloud::drive().is_some_and(|drive| {
        shareable
            && !lines.is_empty()
            && lines
                .iter()
                .all(|line| Path::new(line.key.as_ref()).starts_with(&drive))
    });
    let roots = crate::data::volume_roots();
    let ejectable = lines
        .iter()
        .filter_map(|line| line.row.opens)
        .chain([here])
        .any(|id| id != "/" && roots.contains(&id));
    let worked = Rc::new(Chosen {
        seen,
        count: lines.len(),
        folders: lines.iter().filter(|line| line.row.opens.is_some()).count(),
        refused,
        shareable,
        cloud,
        original: lines.iter().any(|line| {
            matches!(
                line.row.uti.as_ref(),
                "public.symlink" | "com.apple.alias-file"
            )
        }),
        ejectable,
        name: lines
            .first()
            .map(|line| line.row.display.clone())
            .unwrap_or_default(),
        here: finder_fs::name::of_path(Path::new(here)).into(),
        folder: demo
            .folder()
            .map(|folder| folder.title)
            .unwrap_or_default()
            .into(),
        on_disk: Path::new(demo.target()).is_absolute(),
        files: file_paths(&lines),
        keys: lines.into_iter().map(|line| line.key).collect(),
    });
    WORKED.set(worked.clone());
    worked
}

pub(crate) fn remember(demo: &FinderDemo, cx: &App) {
    super::edit::remember(demo);
    CHOSEN.set(chosen(demo));
    MIRROR.set(Mirror {
        view: demo.view,
        sort: demo.sort,
        arranged: demo.arrangement().filter(|_| demo.view == ICON_VIEW),
        cleans_up: demo.cleans_up(),
        grouped: demo.group().is_some(),
        in_trash: is_trash(demo.here()),
        location: crate::locations::is_location(demo.here()),
        info: !demo.results() && demo.here() != SHARED,
        back: demo.at > 0,
        forward: demo.at + 1 < demo.history.len(),
        busy: !demo.jobs.is_empty(),
        preview: demo.preview,
        path_bar: demo.path_bar_shown,
        status_bar: demo.status_bar_shown,
        options: crate::options::shown(cx),
    });
}

fn one_of(action: &dyn Action, rows: &[&dyn Action]) -> bool {
    rows.iter().any(|row| action.partial_eq(*row))
}

fn enabled(action: &dyn Action) -> bool {
    if action.partial_eq(&ToggleDefaultBrowser) {
        return crate::file_viewer::can_be_default();
    }
    if action.partial_eq(&Acknowledgements) {
        return notices().is_some();
    }
    let mirror = MIRROR.get();
    if mirror.location && one_of(action, &[&NewFolder, &Paste, &MoveHere, &PasteExactly]) {
        return false;
    }
    CHOSEN.with_borrow(|chosen| {
        if let Some(on) = super::edit::enabled(action, chosen) {
            return on;
        }
        let picked = chosen.count > 0;
        let files_only = picked && chosen.folders == 0;
        match () {
            _ if action.partial_eq(&OpenSelection) => !chosen.refused,
            _ if action.partial_eq(&OpenInNewTab) => {
                !chosen.refused && chosen.folders == chosen.count
            }
            _ if action.partial_eq(&OpenAndClose) => picked && !chosen.refused,
            _ if one_of(action, &[&OpenWithRow, &AlwaysOpenWithRow, &Print]) => files_only,
            _ if action.partial_eq(&Share) => picked && chosen.shareable,
            _ if action.partial_eq(&ShowOriginal) => chosen.original,
            _ if one_of(action, &[&GetInfo, &GetSummaryInfo]) => picked || mirror.info,
            _ if one_of(action, &[&EjectSelection, &EjectOne, &EjectAll]) => chosen.ejectable,
            _ if action.partial_eq(&Back) => mirror.back,
            _ if action.partial_eq(&Forward) => mirror.forward,
            _ if action.partial_eq(&CleanUpSelection) => mirror.cleans_up && picked,
            _ if one_of(
                action,
                &[
                    &CleanUp,
                    &CleanUpBy,
                    &CleanUpByName,
                    &CleanUpByKind,
                    &CleanUpByModified,
                    &CleanUpByCreated,
                    &CleanUpBySize,
                    &CleanUpByTags,
                ],
            ) =>
            {
                mirror.cleans_up
            }
            _ if one_of(action, &[&ArrangeNone, &ArrangeGrid]) => mirror.arranged.is_some(),
            _ if action.partial_eq(&UseGroups) => mirror.view != GALLERY_VIEW,
            _ if action.partial_eq(&ShowProgressWindow) => {
                mirror.busy || crate::progress_window::shown()
            }
            _ if action.partial_eq(&NoApps) => false,
            _ => {
                picked
                    || !one_of(
                        action,
                        &[
                            &NewFolderWithSelection,
                            &RenameSelection,
                            &Compress,
                            &CompressNamed,
                            &Duplicate,
                            &DuplicateExactly,
                            &MakeAlias,
                            &Trash,
                            &DeleteImmediately,
                            &DeselectAll,
                        ],
                    )
            }
        }
    })
}

fn checked(action: &dyn Action) -> Option<bool> {
    if action.partial_eq(&ToggleDefaultBrowser) {
        return Some(crate::file_viewer::is_default());
    }
    let Mirror {
        view,
        sort,
        arranged,
        grouped,
        ..
    } = MIRROR.get();
    let sorted = |column| match arranged {
        Some(arrangement) => arrangement == Arrangement::By(column),
        None => sort == column,
    };
    let rows: [(&dyn Action, bool); 15] = [
        (&ViewIcons, view == ICON_VIEW),
        (&ViewList, view == LIST_VIEW),
        (&ViewColumns, view == COLUMN_VIEW),
        (&ViewGallery, view == GALLERY_VIEW),
        (&SortByName, sorted(Column::Name)),
        (&SortByKind, sorted(Column::Kind)),
        (&SortByOpened, sorted(Column::Opened)),
        (&SortByAdded, sorted(Column::Added)),
        (&SortByModified, sorted(Column::Modified)),
        (&SortByCreated, sorted(Column::Created)),
        (&SortBySize, sorted(Column::Size)),
        (&SortByTags, sorted(Column::Tags)),
        (&ArrangeNone, arranged == Some(Arrangement::Free)),
        (&ArrangeGrid, arranged == Some(Arrangement::Grid)),
        (&UseGroups, grouped),
    ];
    rows.into_iter()
        .find(|(row, _)| action.partial_eq(*row))
        .map(|(_, on)| on)
}

fn flip(shown: bool, what: &str) -> String {
    format!("{} {what}", if shown { "Hide" } else { "Show" })
}

fn titled(action: &dyn Action) -> Option<SharedString> {
    let mirror = MIRROR.get();
    CHOSEN.with_borrow(|chosen| {
        if let Some(title) = super::edit::titled(action, chosen) {
            return Some(title);
        }
        let title = match () {
            _ if action.partial_eq(&ShowPreviewOptions) => {
                flip(crate::preview::options::is_open(), "Preview Options")
            }
            _ if action.partial_eq(&Trash) => match mirror.in_trash {
                true => "Put Back".into(),
                false => "Move to Trash".into(),
            },
            _ if action.partial_eq(&NewFolderWithSelection) => match chosen.count {
                0 => "New Folder with Selection".into(),
                1 => format!("New Folder with \u{201c}{}\u{201d}", chosen.name),
                many => format!("New Folder with Selection ({many} Items)"),
            },
            _ if action.partial_eq(&OpenInNewTab) => match crate::prefs::opens_folders_in_tabs() {
                true => "Open in New Tab".into(),
                false => "Open in New Window".into(),
            },
            _ if action.partial_eq(&OpenAndClose) => match chosen.count > chosen.folders {
                true => "Open and Close Window".into(),
                false => "Open in New Window and Close".into(),
            },
            _ if action.partial_eq(&CompressNamed) => match chosen.count {
                0 => "Compress".into(),
                1 => format!("Compress \u{201c}{}\u{201d}", chosen.name),
                many => format!("Compress {many} Items"),
            },
            _ if action.partial_eq(&QuickLook) && native_gpui::quick_look_visible() => {
                "Close Quick Look".into()
            }
            _ if action.partial_eq(&Slideshow) && native_gpui::quick_look_visible() => {
                "Slideshow".into()
            }
            _ if action.partial_eq(&QuickLook) => match chosen.count {
                0 => format!("Quick Look \u{201c}{}\u{201d}", chosen.here),
                1 => "Quick Look".into(),
                many => format!("Quick Look {many} Items"),
            },
            _ if action.partial_eq(&Slideshow) => match chosen.count {
                0 => format!("Slideshow \u{201c}{}\u{201d}", chosen.here),
                1 => format!("Slideshow \u{201c}{}\u{201d}", chosen.name),
                many => format!("Slideshow {many} Items"),
            },
            _ if action.partial_eq(&TogglePreview) => flip(mirror.preview, "Preview"),
            _ if action.partial_eq(&TogglePathBar) => flip(mirror.path_bar, "Path Bar"),
            _ if action.partial_eq(&ToggleStatusBar) => flip(mirror.status_bar, "Status Bar"),
            _ if action.partial_eq(&ShowInspector) => {
                flip(crate::inspector::is_open(), "Inspector")
            }
            _ if action.partial_eq(&ShowViewOptions) => flip(mirror.options, "View Options"),
            _ if action.partial_eq(&ShowProgressWindow) => {
                flip(crate::progress_window::shown(), "Progress Window")
            }
            _ => return None,
        };
        Some(title.into())
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
    cx.on_action(crate::progress_window::toggle);
    cx.on_action(|_: &crate::ops::undo::UndoOp, cx| crate::icon_well::undo_in_key_window(cx));
    cx.on_action(|_: &crate::ops::undo::RedoOp, cx| crate::icon_well::redo_in_key_window(cx));
    cx.on_action(|_: &ShowSettings, cx| crate::settings::open(cx));
    cx.on_action(|_: &ShowAbout, cx| crate::about::show(cx));
    cx.on_action(|_: &ToggleDefaultBrowser, _| crate::file_viewer::toggle_default());
    cx.on_action(|_: &NewWindow, cx| {
        open_first_window(cx);
    });
    cx.on_action(|_: &GoToFolder, cx| {
        open_first_window(cx)
            .update(cx, |demo, window, cx| demo.open_go(window, cx))
            .ok();
    });
    follow_locations(cx);
    // A key panel is main too, so gpui has no active window to dispatch the menu's action to.
    cx.on_action(|_: &ShowPreviewOptions, cx| crate::preview::options::close(cx));
    cx.on_action(|_: &MacUserGuide, _| open_mac_help("mh43558"));
    cx.on_action(|_: &TipsForYourMac, cx| cx.open_url(TIPS));
    cx.on_action(|_: &Acknowledgements, _| {
        if let Some(url) = notices() {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    });
    cx.bind_keys([KeyBinding::new("cmd-shift-.", ToggleHidden, None)]);
    on_menu_state(checked);
    on_menu_enabled(enabled);
    on_menu_title(titled);
    set_menu("File", file_rows());
    replace_about_row(ShowAbout);
    extend_menu(
        MenuSection::App,
        [
            MenuCommand::new("Settings…", ShowSettings).key("cmd-,"),
            MenuCommand::separator(),
            MenuCommand::new("Use Found'er as Default File Browser", ToggleDefaultBrowser),
            MenuCommand::separator(),
            MenuCommand::new("Empty Trash…", EmptyTrash).key("cmd-shift-backspace"),
            MenuCommand::new("Empty Trash", EmptyTrashNow)
                .key("cmd-alt-shift-backspace")
                .option_alternate(),
        ],
    );
    set_menu("Window", window_rows());
    set_menu(
        "Help",
        [
            MenuCommand::new("Mac User Guide", MacUserGuide),
            MenuCommand::new("Tips for Your Mac", TipsForYourMac),
            MenuCommand::separator(),
            MenuCommand::new("Acknowledgements", Acknowledgements),
        ],
    );
    replace_edit_rows(super::edit::rows());
    set_menu("View", view_rows());
    add_menu("Go", go_rows(&[]));
}

const TIPS: &str = "x-apple-tips://open?referrer=HelpMenu";

pub(crate) fn open_mac_help(anchor: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSHelpManager::sharedHelpManager(mtm).openHelpAnchor_inBook(
        &NSString::from_str(anchor),
        Some(&NSString::from_str("com.apple.machelp")),
    );
}

fn notices() -> Option<Retained<NSURL>> {
    NSBundle::mainBundle().URLForResource_withExtension(
        Some(ns_string!("THIRD_PARTY_NOTICES")),
        Some(ns_string!("html")),
    )
}

fn file_rows() -> Vec<MenuCommand> {
    vec![
        MenuCommand::new("New Finder Window", NewWindow).key("cmd-n"),
        MenuCommand::new("New Folder", NewFolder).key("cmd-shift-n"),
        MenuCommand::new("New Folder with Selection", NewFolderWithSelection).key("cmd-ctrl-n"),
        MenuCommand::new("New Smart Folder", NewSmartFolder),
        MenuCommand::appkit("New Tab", c"newWindowForTab:").key("cmd-t"),
        MenuCommand::new("Open", OpenSelection).key("cmd-o"),
        MenuCommand::new("Open in New Tab", OpenInNewTab)
            .key("cmd-ctrl-o")
            .option_alternate(),
        MenuCommand::new("Open and Close Window", OpenAndClose)
            .key("cmd-alt-o")
            .option_alternate(),
        MenuCommand::new("Open With", OpenWithRow).filled(|| open_with_rows(false)),
        MenuCommand::new("Always Open With", AlwaysOpenWithRow)
            .filled(|| open_with_rows(true))
            .option_alternate(),
        MenuCommand::appkit("Close Window", c"performClose:").key("cmd-w"),
        MenuCommand::separator(),
        MenuCommand::new("Get Info", GetInfo).key("cmd-i"),
        MenuCommand::new("Show Inspector", ShowInspector)
            .key("cmd-alt-i")
            .option_alternate(),
        MenuCommand::new("Get Summary Info", GetSummaryInfo)
            .key("cmd-ctrl-i")
            .option_alternate(),
        MenuCommand::new("Rename", RenameSelection),
        MenuCommand::new("Compress", Compress),
        MenuCommand::new("Compress", CompressNamed).option_alternate(),
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
        MenuCommand::new("Add to Dock", AddToDock)
            .key("cmd-ctrl-shift-t")
            .option_alternate(),
        MenuCommand::separator(),
        MenuCommand::new("Move to Trash", Trash).key("cmd-backspace"),
        MenuCommand::new("Delete Immediately…", DeleteImmediately)
            .key("cmd-alt-backspace")
            .option_alternate(),
        MenuCommand::new("Eject", EjectSelection).key("cmd-e"),
        MenuCommand::new("Eject", EjectOne)
            .key("cmd-ctrl-e")
            .option_alternate(),
        MenuCommand::new("Eject", EjectAll)
            .key("cmd-alt-e")
            .option_alternate(),
        MenuCommand::separator(),
        MenuCommand::view(tag_row, EditTags),
        MenuCommand::separator(),
        MenuCommand::new("Print", Print).key("cmd-p"),
    ]
}

fn tag_row() -> native_gpui::MenuView {
    let chosen = CHOSEN.with_borrow(Rc::clone);
    let carried: Vec<_> = chosen
        .keys
        .iter()
        .map(|key| crate::tags::row_tags(key))
        .collect();
    swatch_row(&carried, chosen.count == 1 && chosen.folders == 1)
}

fn open_with_rows(always: bool) -> Vec<MenuCommand> {
    let chosen = CHOSEN.with_borrow(Rc::clone);
    let (default, rest): (Vec<_>, Vec<_>) = crate::openwith::apps_for_all(&chosen.files)
        .into_iter()
        .partition(|app| app.default);
    let row = |title: String, app: PathBuf| MenuCommand::new(title, OpenWithApp { app, always });
    let mut rows: Vec<MenuCommand> = default
        .into_iter()
        .map(|app| row(format!("{} (default)", app.name), app.path))
        .collect();
    if !rows.is_empty() && !rest.is_empty() {
        rows.push(MenuCommand::separator());
    }
    rows.extend(rest.into_iter().map(|app| row(app.name, app.path)));
    if rows.is_empty() {
        rows.push(MenuCommand::new("<None>", NoApps));
    }
    rows.push(MenuCommand::separator());
    rows.push(MenuCommand::new("App Store…", OpenWithStore));
    rows.push(MenuCommand::new("Other…", OpenWithOther { always }));
    rows
}

fn view_rows() -> Vec<MenuCommand> {
    vec![
        MenuCommand::new("as Icons", ViewIcons).key("cmd-1"),
        MenuCommand::new("as List", ViewList).key("cmd-2"),
        MenuCommand::new("as Columns", ViewColumns).key("cmd-3"),
        MenuCommand::new("as Gallery", ViewGallery).key("cmd-4"),
        MenuCommand::separator(),
        MenuCommand::new("Use Groups", UseGroups).key("cmd-ctrl-0"),
        MenuCommand::submenu("Sort By", []).filled(sort_by_rows),
        MenuCommand::new("Clean Up", CleanUp),
        MenuCommand::new("Clean Up Selection", CleanUpSelection).option_alternate(),
        MenuCommand::new("Clean Up By", CleanUpBy).rows([
            MenuCommand::new("Name", CleanUpByName).key("cmd-alt-1"),
            MenuCommand::new("Kind", CleanUpByKind).key("cmd-alt-2"),
            MenuCommand::new("Date Modified", CleanUpByModified).key("cmd-alt-5"),
            MenuCommand::new("Date Created", CleanUpByCreated),
            MenuCommand::new("Size", CleanUpBySize).key("cmd-alt-6"),
            MenuCommand::new("Tags", CleanUpByTags).key("cmd-alt-7"),
        ]),
        MenuCommand::separator(),
        MenuCommand::appkit("Show Tab Bar", c"toggleTabBar:").key("cmd-shift-t"),
        MenuCommand::appkit("Show All Tabs", c"toggleTabOverview:").key("cmd-shift-\\"),
        MenuCommand::separator(),
        MenuCommand::appkit("Show Sidebar", c"toggleSidebar:").key("cmd-ctrl-s"),
        MenuCommand::new("Show Preview", TogglePreview).key("cmd-shift-p"),
        MenuCommand::separator(),
        MenuCommand::appkit("Show Toolbar", c"toggleToolbarShown:").key("cmd-alt-t"),
        MenuCommand::new("Show Path Bar", TogglePathBar).key("cmd-alt-p"),
        MenuCommand::new("Show Status Bar", ToggleStatusBar).key("cmd-/"),
        MenuCommand::separator(),
        MenuCommand::appkit("Customize Toolbar…", c"runToolbarCustomizationPalette:"),
        MenuCommand::separator(),
        MenuCommand::new("Show View Options", ShowViewOptions).key("cmd-j"),
        MenuCommand::new("Show Preview Options", ShowPreviewOptions),
        MenuCommand::separator(),
        MenuCommand::appkit("Enter Full Screen", c"toggleFullScreen:").key("cmd-ctrl-f"),
    ]
}

fn sort_by_rows() -> Vec<MenuCommand> {
    let mut rows = Vec::new();
    if MIRROR.get().view == ICON_VIEW {
        rows.extend([
            MenuCommand::new("None", ArrangeNone).key("cmd-ctrl-alt-0"),
            MenuCommand::separator(),
            MenuCommand::new("Snap to Grid", ArrangeGrid),
            MenuCommand::separator(),
        ]);
    }
    rows.extend([
        MenuCommand::new("Name", SortByName).key("cmd-ctrl-alt-1"),
        MenuCommand::new("Kind", SortByKind).key("cmd-ctrl-alt-2"),
        MenuCommand::new("Date Last Opened", SortByOpened).key("cmd-ctrl-alt-3"),
        MenuCommand::new("Date Added", SortByAdded).key("cmd-ctrl-alt-4"),
        MenuCommand::new("Date Modified", SortByModified).key("cmd-ctrl-alt-5"),
        MenuCommand::new("Date Created", SortByCreated),
        MenuCommand::new("Size", SortBySize).key("cmd-ctrl-alt-6"),
        MenuCommand::new("Tags", SortByTags).key("cmd-ctrl-alt-7"),
    ]);
    rows
}

fn window_rows() -> Vec<MenuCommand> {
    vec![
        MenuCommand::appkit("Minimize", c"performMiniaturize:").key("cmd-m"),
        MenuCommand::appkit("Zoom", c"performZoom:"),
        MenuCommand::separator(),
        MenuCommand::appkit("Bring All to Front", c"arrangeInFront:"),
        MenuCommand::new("Cycle Through Windows", CycleWindows).key("cmd-`"),
        MenuCommand::separator(),
        MenuCommand::appkit("Show Previous Tab", c"selectPreviousTab:")
            .key("ctrl-shift-tab")
            .to_main_window(),
        MenuCommand::appkit("Show Next Tab", c"selectNextTab:")
            .key("ctrl-tab")
            .to_main_window(),
        MenuCommand::appkit("Move Tab to New Window", c"moveTabToNewWindow:").to_main_window(),
        MenuCommand::appkit("Merge All Windows", c"mergeAllWindows:").to_main_window(),
        MenuCommand::separator(),
        MenuCommand::new("Show Progress Window", ShowProgressWindow),
    ]
}

pub(crate) fn go_rows(recents: &[(&'static str, String)]) -> Vec<MenuCommand> {
    let mut recent_rows: Vec<MenuCommand> = recents
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, (_, title))| MenuCommand::new(title.clone(), GoRecent(index)))
        .collect();
    if !recent_rows.is_empty() {
        recent_rows.push(MenuCommand::separator());
    }
    recent_rows.push(MenuCommand::new("Clear Menu", ClearRecents));
    let mut rows = go_head();
    rows.extend([
        MenuCommand::separator(),
        MenuCommand::submenu("Recent Folders", recent_rows),
        MenuCommand::separator(),
        MenuCommand::new("Go to Folder…", GoToFolder).key("cmd-shift-g"),
        MenuCommand::new("Connect to Server…", ConnectToServer).key("cmd-k"),
    ]);
    rows
}

/// Listing locations asks each volume for its capacity, which an unresponsive share stalls.
fn follow_locations(cx: &mut App) {
    cx.spawn(async move |cx| {
        let found = cx
            .background_executor()
            .spawn(async {
                listed_locations()
                    .into_iter()
                    .filter(|row| row.special == Some(Special::FileProvider))
                    .filter_map(|row| Some((row.title, intern_path(&row.path?))))
                    .collect::<Vec<_>>()
            })
            .await;
        cx.update(|cx| {
            LOCATIONS.set(found);
            let recents = crate::navigate::browser_windows(cx)
                .first()
                .and_then(|front| front.read(cx).ok().map(|demo| demo.recents.clone()))
                .unwrap_or_default();
            set_menu("Go", go_rows(&recents));
        });
    })
    .detach();
}

fn go_head() -> Vec<MenuCommand> {
    let mut rows = vec![
        MenuCommand::new("Back", Back).key("cmd-["),
        MenuCommand::new("Forward", Forward).key("cmd-]"),
        MenuCommand::new("Enclosing Folder", Up).key("cmd-up"),
        MenuCommand::new("Enclosing Folder", Up)
            .key("cmd-alt-up")
            .option_alternate(),
        MenuCommand::new("Enclosing Folder in New Window", UpInNewWindow)
            .key("cmd-ctrl-up")
            .option_alternate(),
        MenuCommand::new("Select Startup Disk", SelectStartupDisk)
            .key("cmd-shift-up")
            .option_alternate(),
        MenuCommand::separator(),
        MenuCommand::new("Recents", GoRecents).key("cmd-shift-f"),
        MenuCommand::new("Documents", GoDocuments).key("cmd-shift-o"),
        MenuCommand::new("Desktop", GoDesktop).key("cmd-shift-d"),
        MenuCommand::new("Downloads", GoDownloads).key("cmd-alt-l"),
        MenuCommand::new("Home", GoHome).key("cmd-shift-h"),
        MenuCommand::new("Library", GoLibrary).option_alternate(),
        MenuCommand::new("Computer", GoComputer).key("cmd-shift-c"),
        MenuCommand::new("AirDrop", GoAirDrop).key("cmd-shift-r"),
        MenuCommand::new("Network", GoNetwork).key("cmd-shift-k"),
        MenuCommand::new("iCloud Drive", GoICloud).key("cmd-shift-i"),
        MenuCommand::new("Shared", GoShared).key("cmd-shift-s"),
        MenuCommand::new("Applications", GoApplications).key("cmd-shift-a"),
        MenuCommand::new("Utilities", GoUtilities).key("cmd-shift-u"),
    ];
    LOCATIONS.with_borrow(|known| {
        rows.extend(
            known
                .iter()
                .map(|(title, path)| MenuCommand::new(title.clone(), GoLocation(*path))),
        );
    });
    rows
}

fn run_act<A: Action>(
    act: Act,
) -> impl Fn(&mut FinderDemo, &A, &mut Window, &mut Context<FinderDemo>) + 'static {
    move |this, _, window, cx| this.run_menu(act, window, cx)
}

fn file_paths(lines: &[Line]) -> Vec<PathBuf> {
    lines
        .iter()
        .filter(|line| line.row.opens.is_none())
        .map(|line| PathBuf::from(line.key.as_ref()))
        .collect()
}

impl FinderDemo {
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

    fn chosen_files(&self) -> Vec<PathBuf> {
        file_paths(&self.chosen_lines())
    }

    /// This volume only, not the others on its disk.
    fn eject_only(&mut self) {
        let Some(volume) = self.volumes_in_view().find(|id| *id != "/") else {
            return;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(volume));
        let refused = RcBlock::new(|error: *mut NSError| {
            let Some(error) = (unsafe { error.as_ref() }) else {
                return;
            };
            let text = error.localizedDescription();
            let complain = RcBlock::new(move || {
                if let Some(mtm) = MainThreadMarker::new() {
                    let alert = NSAlert::new(mtm);
                    alert.setMessageText(&text);
                    alert.runModal();
                }
            });
            unsafe { NSRunLoop::mainRunLoop().performBlock(&complain) };
        });
        unsafe {
            NSFileManager::defaultManager().unmountVolumeAtURL_options_completionHandler(
                &url,
                NSFileManagerUnmountOptions::empty(),
                &refused,
            )
        };
    }

    pub(crate) fn menu_bar_actions(&mut self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .on_action(cx.listener(|this, _: &NewWindow, _, cx| this.open_new_window(cx)))
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                this.open_tab(super::actions::new_window_target(), window, cx);
            }))
            .on_action(cx.listener(run_act::<NewFolder>(Act::NewFolder)))
            .on_action(cx.listener(run_act::<NewFolderWithSelection>(Act::NewFolderWith)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(Pasting::Copy, cx)))
            .on_action(cx.listener(|this, _: &MoveHere, _, cx| this.paste(Pasting::Move, cx)))
            .on_action(cx.listener(|this, _: &PasteExactly, _, cx| {
                this.paste(Pasting::Exactly, cx);
            }))
            .on_action(cx.listener(run_act::<OpenSelection>(Act::Open)))
            .on_action(cx.listener(|this, _: &OpenInNewTab, window, cx| {
                let mut folders = this.chosen_folders();
                if folders.is_empty() {
                    folders.push(this.here());
                }
                match crate::prefs::opens_folders_in_tabs() {
                    true => folders
                        .into_iter()
                        .for_each(|id| this.open_tab(id, window, cx)),
                    false => open_windows(folders, cx),
                }
            }))
            .on_action(cx.listener(|this, _: &OpenAndClose, window, cx| {
                let lines = this.chosen_lines();
                if lines.is_empty() {
                    return;
                }
                crate::openwith::open_default(&file_paths(&lines));
                open_windows(lines.iter().filter_map(|line| line.row.opens), cx);
                window.remove_window();
            }))
            .on_action(cx.listener(|this, action: &OpenWithApp, _, _| {
                let files = this.chosen_files();
                if action.always {
                    crate::openwith::set_default(&files, &action.app);
                }
                crate::openwith::open_with(&files, &action.app);
            }))
            .on_action(cx.listener(|this, _: &OpenWithStore, _, _| {
                crate::openwith::app_store(&this.chosen_files());
            }))
            .on_action(cx.listener(|this, action: &OpenWithOther, _, cx| {
                crate::openwith::choose_app(this.chosen_files(), action.always, cx);
            }))
            .on_action(cx.listener(run_act::<MakeAlias>(Act::MakeAlias)))
            .on_action(cx.listener(run_act::<Compress>(Act::Compress)))
            .on_action(cx.listener(run_act::<CompressNamed>(Act::Compress)))
            .on_action(cx.listener(run_act::<ShowOriginal>(Act::ShowOriginal)))
            .on_action(cx.listener(|this, _: &AddToSidebar, _, cx| {
                this.add_to_sidebar(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &AddToDock, _, _| {
                let mut paths = this.paths();
                if paths.is_empty() {
                    paths.push(PathBuf::from(this.here()));
                }
                if let Err(error) = crate::dock::add(&paths) {
                    this.complain(&error);
                }
            }))
            .on_action(cx.listener(|this, _: &EjectSelection, window, cx| {
                this.eject_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &EjectOne, _, _| this.eject_only()))
            .on_action(cx.listener(|this, _: &EjectAll, window, cx| {
                this.eject_selection(window, cx);
            }))
            .on_action(cx.listener(|this, _: &EmptyTrash, _, cx| this.empty_trash(cx)))
            .on_action(cx.listener(|this, _: &EmptyTrashNow, _, cx| this.empty_trash_now(cx)))
            .on_action(cx.listener(|this, _: &FindHere, window, cx| {
                set_toolbar_search_active(window, SEARCH_ID, true);
                this.search_editing = true;
                if this.criteria.is_empty() {
                    this.criteria.push(Criterion::default());
                    if this.in_search() {
                        this.run_search(cx);
                    }
                }
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
            .on_action(cx.listener(run_act::<GetInfo>(Act::GetInfo)))
            .on_action(cx.listener(run_act::<GetSummaryInfo>(Act::SummaryInfo)))
            .on_action(cx.listener(run_act::<ShowPreviewOptions>(Act::PreviewOptions)))
            .on_action(cx.listener(run_act::<ShowInspector>(Act::Inspector)))
            .on_action(cx.listener(run_act::<RenameSelection>(Act::Rename)))
            .on_action(cx.listener(run_act::<Duplicate>(Act::Duplicate)))
            .on_action(cx.listener(run_act::<DuplicateExactly>(Act::Duplicate)))
            .on_action(cx.listener(run_act::<DeleteImmediately>(Act::DeleteNow)))
            .on_action(cx.listener(run_act::<Trash>(Act::Trash)))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| this.copy_pathnames(cx)))
            .on_action(cx.listener(|this, _: &DeselectAll, _, cx| {
                this.selected.clear();
                cx.notify();
            }))
            .on_action(cx.listener(run_act::<ViewIcons>(Act::View(ICON_VIEW))))
            .on_action(cx.listener(run_act::<ViewList>(Act::View(LIST_VIEW))))
            .on_action(cx.listener(run_act::<ViewColumns>(Act::View(COLUMN_VIEW))))
            .on_action(cx.listener(run_act::<ViewGallery>(Act::View(GALLERY_VIEW))))
            .on_action(cx.listener(run_act::<SortByName>(Act::Sort(Column::Name))))
            .on_action(cx.listener(run_act::<SortByKind>(Act::Sort(Column::Kind))))
            .on_action(cx.listener(run_act::<SortByOpened>(Act::Sort(Column::Opened))))
            .on_action(cx.listener(run_act::<SortByAdded>(Act::Sort(Column::Added))))
            .on_action(cx.listener(run_act::<SortByModified>(Act::Sort(Column::Modified))))
            .on_action(cx.listener(run_act::<SortByCreated>(Act::Sort(Column::Created))))
            .on_action(cx.listener(run_act::<SortBySize>(Act::Sort(Column::Size))))
            .on_action(cx.listener(run_act::<SortByTags>(Act::Sort(Column::Tags))))
            .on_action(cx.listener(run_act::<ShowViewOptions>(Act::ViewOptions)))
            .on_action(cx.listener(run_act::<UseGroups>(Act::Groups)))
            .on_action(cx.listener(|this, _: &CleanUp, _, cx| this.clean_up(false, cx)))
            .on_action(cx.listener(|this, _: &CleanUpSelection, _, cx| this.clean_up(true, cx)))
            .on_action(cx.listener(run_act::<CleanUpByName>(Act::CleanUpBy(Column::Name))))
            .on_action(cx.listener(run_act::<CleanUpByKind>(Act::CleanUpBy(Column::Kind))))
            .on_action(cx.listener(run_act::<CleanUpByModified>(Act::CleanUpBy(
                Column::Modified,
            ))))
            .on_action(cx.listener(run_act::<CleanUpByCreated>(Act::CleanUpBy(Column::Created))))
            .on_action(cx.listener(run_act::<CleanUpBySize>(Act::CleanUpBy(Column::Size))))
            .on_action(cx.listener(run_act::<CleanUpByTags>(Act::CleanUpBy(Column::Tags))))
            .on_action(cx.listener(run_act::<ArrangeNone>(Act::Arrange(Arrangement::Free))))
            .on_action(cx.listener(run_act::<ArrangeGrid>(Act::Arrange(Arrangement::Grid))))
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
                crate::prefs::set_shows_path_bar(this.path_bar_shown);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleStatusBar, window, cx| {
                this.toggle_status_bar(window, cx);
            }))
            .on_action(cx.listener(|_, _: &ToggleHidden, _, cx| {
                cx.defer(crate::listing::toggle_hidden);
            }))
            .on_action(cx.listener(|this, _: &Up, window, cx| this.up(window, cx)))
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
                this.go(COMPUTER, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoNetwork, window, cx| this.go(NETWORK, window, cx)))
            .on_action(cx.listener(|this, _: &GoShared, window, cx| this.go(SHARED, window, cx)))
            .on_action(cx.listener(|this, _: &GoUtilities, window, cx| {
                this.go_to(Path::new("/Applications/Utilities"), window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoAirDrop, window, cx| this.go(AIRDROP, window, cx)))
            .on_action(cx.listener(|this, _: &GoICloud, window, cx| {
                if let Some(drive) = finder_fs::icloud::drive() {
                    this.go_to(&drive, window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, GoLocation(path): &GoLocation, window, cx| {
                    this.go_to(Path::new(path), window, cx);
                }),
            )
            .on_action(cx.listener(|this, GoRecent(index): &GoRecent, window, cx| {
                if let Some(&(id, _)) = this.recents.get(*index) {
                    this.go(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ClearRecents, _, _| {
                this.recents.clear();
                set_menu("Go", go_rows(&this.recents));
            }))
            .on_action(cx.listener(|this, _: &GoToFolder, window, cx| this.open_go(window, cx)))
            .on_action(cx.listener(|_, _: &ConnectToServer, window, cx| {
                let demo = cx.entity().downgrade();
                crate::chrome::connect::show(demo, window.window_handle(), cx);
            }))
            .on_action(cx.listener(run_act::<QuickLook>(Act::QuickLook)))
            .on_action(cx.listener(|this, _: &Slideshow, window, cx| this.slideshow(window, cx)))
            .on_action(cx.listener(|this, _: &Print, _, _| {
                if let Err(error) = crate::openwith::print(&this.paths()) {
                    this.complain(&error);
                }
            }))
            .on_action(cx.listener(run_act::<Share>(Act::Share)))
            .on_action(cx.listener(run_act::<EditTags>(Act::Swatch)))
    }
}
