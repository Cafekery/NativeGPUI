//! The menu bar Finder carries, read off the real one: the same rows, order and key equivalents.

use finder_fs::volumes::{self, Standard};
use gpui::{App, Context, Div, InteractiveElement, Window, actions};
use native_gpui::{MenuCommand, MenuSection, add_menu, extend_menu, prepend_menu};

use crate::data::{Column, intern_path};
use crate::menus::Act;
use crate::{COLUMN_VIEW, FinderDemo, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};

actions!(
    finder_demo,
    [
        Back,
        CleanUp,
        UseGroups,
        ShowViewOptions,
        Cut,
        Paste,
        MoveHere,
        CopyNames,
        CopyPath,
        DeselectAll,
        Duplicate,
        DuplicateExactly,
        Forward,
        GetInfo,
        GoApplications,
        GoDesktop,
        GoDocuments,
        GoDownloads,
        GoHome,
        GoToFolder,
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
        OpenSelection,
        QuickLook,
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
        DeleteImmediately,
        Up,
        ViewColumns,
        ViewGallery,
        ViewIcons,
        ViewList,
        ZoomIn,
        ZoomOut,
    ]
);

pub(crate) fn install(_cx: &mut App) {
    prepend_menu(
        MenuSection::File,
        [
            MenuCommand::new("New Finder Window", NewWindow).key("cmd-n"),
            MenuCommand::new("New Folder", NewFolder).key("cmd-shift-n"),
            MenuCommand::new("New Text File", NewTextFile),
            MenuCommand::new("New Folder with Selection", NewFolderWithSelection).key("cmd-ctrl-n"),
            MenuCommand::separator(),
            MenuCommand::new("Open", OpenSelection).key("cmd-o"),
            MenuCommand::new("Open in New Window", OpenInNewWindow).key("cmd-ctrl-o"),
            MenuCommand::separator(),
            MenuCommand::new("Get Info", GetInfo).key("cmd-i"),
            MenuCommand::new("Show Inspector", ShowInspector).key("cmd-alt-i"),
            MenuCommand::new("Rename", RenameSelection),
            MenuCommand::new("Duplicate", Duplicate).key("cmd-d"),
            MenuCommand::new("Duplicate Exactly", DuplicateExactly)
                .key("cmd-alt-d")
                .option_alternate(),
            MenuCommand::new("Quick Look", QuickLook).key("cmd-y"),
            MenuCommand::separator(),
            MenuCommand::new("Move to Trash", Trash).key("cmd-backspace"),
            MenuCommand::new("Delete Immediately…", DeleteImmediately)
                .key("cmd-alt-backspace")
                .option_alternate(),
            MenuCommand::separator(),
        ],
    );
    extend_menu(
        MenuSection::Edit,
        [
            MenuCommand::separator(),
            MenuCommand::new("Move Item Here", MoveHere).key("cmd-alt-v"),
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
                    MenuCommand::new("Name", SortByName).key("cmd-ctrl-1"),
                    MenuCommand::new("Kind", SortByKind).key("cmd-ctrl-2"),
                    MenuCommand::new("Date Modified", SortByModified).key("cmd-ctrl-3"),
                    MenuCommand::new("Size", SortBySize).key("cmd-ctrl-5"),
                ],
            ),
            MenuCommand::new("Use Groups", UseGroups).key("cmd-ctrl-0"),
            MenuCommand::new("Show View Options", ShowViewOptions).key("cmd-j"),
            MenuCommand::new("Clean Up", CleanUp),
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

/// Finder's Go menu, with the folders the window has been through under Recent Folders.
pub(crate) fn go_rows(recents: &[(&'static str, String)]) -> Vec<MenuCommand> {
    let recent_rows: Vec<MenuCommand> = recents
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
    let mut recent_rows = recent_rows;
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
        MenuCommand::separator(),
        MenuCommand::new("Documents", GoDocuments).key("cmd-shift-o"),
        MenuCommand::new("Desktop", GoDesktop).key("cmd-shift-d"),
        MenuCommand::new("Downloads", GoDownloads).key("cmd-alt-l"),
        MenuCommand::new("Home", GoHome).key("cmd-shift-h"),
        MenuCommand::new("Applications", GoApplications).key("cmd-shift-a"),
        MenuCommand::separator(),
        MenuCommand::new("Go to Folder…", GoToFolder).key("cmd-shift-g"),
        MenuCommand::separator(),
    ]
}

/// One handler per row: everything the bar dispatches, in the order the menus list it.
impl FinderDemo {
    fn go_recent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.recents.get(index).map(|(id, _)| *id) {
            self.go(id, window, cx);
        }
    }

    fn go_place(&mut self, which: Standard, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = volumes::standard(which) {
            self.go(intern_path(&path), window, cx);
        }
    }

    pub(crate) fn menu_bar_actions(&mut self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .on_action(cx.listener(|this, _: &NewWindow, _, cx| {
                crate::open_finder_window(this.here(), 1, cx);
            }))
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                this.open_tab(this.here(), window, cx);
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
            .on_action(cx.listener(|this, _: &Cut, _, cx| {
                this.cut_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                this.paste(false, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveHere, _, cx| {
                this.paste(true, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSelection, window, cx| {
                this.run_menu(Act::Open, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenInNewWindow, window, cx| {
                this.run_menu(Act::OpenWindow, window, cx);
            }))
            .on_action(cx.listener(|_, _: &GetInfo, _, cx| {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, None, cx));
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
            .on_action(cx.listener(|_, _: &ShowViewOptions, _, cx| {
                let demo = cx.entity();
                cx.defer(move |cx| crate::options::open(&demo, cx));
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
            .on_action(cx.listener(|this, _: &GoApplications, window, cx| {
                this.go_place(Standard::Applications, window, cx);
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
            .on_action(cx.listener(|this, _: &GoToFolder, _, cx| {
                this.go_sheet = Some(this.crumb_path());
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &QuickLook, window, cx| {
                this.toggle_quick_look(window, cx);
            }))
    }
}
