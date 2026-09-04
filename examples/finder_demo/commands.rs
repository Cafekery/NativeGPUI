//! The menu bar Finder carries, read off the real one: the same rows, order and key equivalents.

use gpui::{App, Context, Div, InteractiveElement, actions};
use native_gpui::{MenuCommand, MenuSection, add_menu, extend_menu, prepend_menu};

use crate::data::Column;
use crate::menus::Act;
use crate::{COLUMN_VIEW, FinderDemo, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};

actions!(
    finder_demo,
    [
        Back,
        CleanUp,
        CopyNames,
        CopyPath,
        DeselectAll,
        Duplicate,
        Forward,
        GetInfo,
        GoApplications,
        GoDesktop,
        GoDocuments,
        GoDownloads,
        GoHome,
        NewFolder,
        NewFolderWithSelection,
        NewWindow,
        OpenSelection,
        OpenInNewWindow,
        RenameSelection,
        SelectAll,
        ShowInspector,
        SortByKind,
        SortByModified,
        SortByName,
        SortBySize,
        ToggleHidden,
        TogglePathBar,
        TogglePreview,
        ToggleSidebar,
        ToggleStatusBar,
        Trash,
        Up,
        ViewColumns,
        ViewGallery,
        ViewIcons,
        ViewList,
    ]
);

pub(crate) fn install(_cx: &mut App) {
    prepend_menu(
        MenuSection::File,
        [
            MenuCommand::new("New Finder Window", NewWindow).key("cmd-n"),
            MenuCommand::new("New Folder", NewFolder).key("cmd-shift-n"),
            MenuCommand::new("New Folder with Selection", NewFolderWithSelection).key("cmd-ctrl-n"),
            MenuCommand::separator(),
            MenuCommand::new("Open", OpenSelection).key("cmd-o"),
            MenuCommand::new("Open in New Window", OpenInNewWindow).key("cmd-ctrl-o"),
            MenuCommand::separator(),
            MenuCommand::new("Get Info", GetInfo).key("cmd-i"),
            MenuCommand::new("Show Inspector", ShowInspector).key("cmd-alt-i"),
            MenuCommand::new("Rename", RenameSelection),
            MenuCommand::new("Duplicate", Duplicate).key("cmd-d"),
            MenuCommand::separator(),
            MenuCommand::new("Move to Trash", Trash).key("cmd-backspace"),
            MenuCommand::separator(),
        ],
    );
    extend_menu(
        MenuSection::Edit,
        [
            MenuCommand::separator(),
            MenuCommand::new("Copy as Pathname", CopyPath).key("cmd-alt-c"),
            MenuCommand::new("Deselect All", DeselectAll).key("cmd-alt-a"),
        ],
    );
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
                    MenuCommand::new("Name", SortByName),
                    MenuCommand::new("Kind", SortByKind),
                    MenuCommand::new("Date Modified", SortByModified),
                    MenuCommand::new("Size", SortBySize),
                ],
            ),
            MenuCommand::new("Clean Up", CleanUp),
            MenuCommand::separator(),
            MenuCommand::new("Show Preview", TogglePreview).key("cmd-shift-p"),
            MenuCommand::new("Show Path Bar", TogglePathBar).key("cmd-alt-p"),
            MenuCommand::new("Show Status Bar", ToggleStatusBar).key("cmd-/"),
            MenuCommand::new("Show Hidden Files", ToggleHidden).key("cmd-shift-."),
            MenuCommand::separator(),
        ],
    );
    add_menu(
        "Go",
        [
            MenuCommand::new("Back", Back).key("cmd-["),
            MenuCommand::new("Forward", Forward).key("cmd-]"),
            MenuCommand::new("Enclosing Folder", Up).key("cmd-up"),
            MenuCommand::separator(),
            MenuCommand::new("Documents", GoDocuments).key("cmd-shift-o"),
            MenuCommand::new("Desktop", GoDesktop).key("cmd-shift-d"),
            MenuCommand::new("Downloads", GoDownloads).key("cmd-alt-l"),
            MenuCommand::new("Home", GoHome).key("cmd-shift-h"),
            MenuCommand::new("Applications", GoApplications).key("cmd-shift-a"),
        ],
    );
}

/// One handler per row: everything the bar dispatches, in the order the menus list it.
impl FinderDemo {
    pub(crate) fn menu_bar_actions(&mut self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .on_action(cx.listener(|this, _: &NewWindow, _, cx| {
                crate::open_finder_window(this.here(), 1, cx);
            }))
            .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
                this.run_menu(Act::NewFolder, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewFolderWithSelection, window, cx| {
                this.run_menu(Act::NewFolder, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSelection, window, cx| {
                this.run_menu(Act::Open, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenInNewWindow, window, cx| {
                this.run_menu(Act::OpenWindow, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GetInfo, window, cx| {
                this.run_menu(Act::Inspector, window, cx);
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
            .on_action(cx.listener(|this, _: &Trash, window, cx| {
                this.run_menu(Act::Trash, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| {
                let path = this.crumb_path();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(path));
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
            .on_action(cx.listener(|this, _: &SortByModified, window, cx| {
                this.run_menu(Act::Sort(Column::Modified), window, cx);
            }))
            .on_action(cx.listener(|this, _: &SortBySize, window, cx| {
                this.run_menu(Act::Sort(Column::Size), window, cx);
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
                this.preview = !this.preview;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TogglePathBar, _, cx| {
                this.path_bar_shown = !this.path_bar_shown;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleStatusBar, _, cx| {
                this.status_bar_shown = !this.status_bar_shown;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleHidden, _, cx| {
                this.show_hidden = !this.show_hidden;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Up, window, cx| {
                this.up(window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoDocuments, window, cx| {
                this.go("documents", window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoDesktop, window, cx| {
                this.go("desktop", window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoDownloads, window, cx| {
                this.go("downloads", window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoHome, window, cx| {
                this.go("home", window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoApplications, window, cx| {
                this.go("applications", window, cx);
            }))
    }
}
