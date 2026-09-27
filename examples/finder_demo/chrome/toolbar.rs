use crate::chrome::sheet::ConnectSheet;
use crate::menus::{Act, Menu};
use block2::RcBlock;
use gpui::{Context, Window, px};
use native_gpui::sidebar::system_template;
use native_gpui::{
    MenuEntry, ToolbarCustomization, ToolbarGroupWidth, ToolbarItem, ToolbarSegment,
    ToolbarSelection, ToolbarStyle, chrome_host, set_customizable_toolbar,
};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSImage, NSMenuToolbarItem, NSToolbarItem, NSWindowTitleVisibility,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;
use std::ptr;
use std::rc::Rc;
use std::sync::LazyLock;

use crate::app::FinderDemo;
use crate::data::{Column, intern_path};

pub(crate) type ToolbarState = (usize, usize, bool, bool, bool, bool, usize, Column, bool);

pub(crate) const GROUPINGS: &[(Option<Column>, &str)] = &[
    (Some(Column::Name), "Name"),
    (Some(Column::Kind), "Kind"),
    (None, "Application"),
    (None, "Date Last Opened"),
    (None, "Date Added"),
    (Some(Column::Modified), "Date Modified"),
    (None, "Date Created"),
    (Some(Column::Size), "Size"),
    (None, "Tags"),
];

const AIRDROP_ART: f32 = 18.;
const AIRDROP_BOX: f64 = 21.;
const GRID_W: f64 = 18.;
const GRID_H: f64 = 17.5;
const GRID_RULE_H: f64 = 1.5;
const GRID_CELL: f64 = 5.;
const GRID_GAP: f64 = 1.5;
const GRID_ROWS: [f64; 4] = [0., 3., 9.5, 12.5];
const GRID_BOX_W: f64 = 22.;

pub(crate) const NAV_ID: &str = "com.apple.finder.BACK";
pub(crate) const AIRDROP_ID: &str = "com.apple.finder.AirD";
pub(crate) const VIEW_ID: &str = "com.apple.finder.SWCH";
pub(crate) const GROUP_ID: &str = "com.apple.finder.ARNG";
pub(crate) const SHARE_ID: &str = "com.apple.finder.SHAR";
pub(crate) const TAG_ID: &str = "com.apple.finder.LABL";
pub(crate) const ACTION_ID: &str = "com.apple.finder.ACTN";
pub(crate) const SEARCH_ID: &str = "com.apple.finder.SRCH";
pub(crate) const JOBS_ID: &str = "ng.finder.jobs";
const PATH_ID: &str = "com.apple.finder.PATH";
const SPACE_ID: &str = "NSToolbarSpaceItem";

const DEFAULTS: [&str; 10] = [
    NAV_ID, VIEW_ID, SPACE_ID, GROUP_ID, SPACE_ID, SHARE_ID, TAG_ID, ACTION_ID, SPACE_ID, SEARCH_ID,
];

type Run = fn(&mut FinderDemo, &mut Window, &mut Context<FinderDemo>);
/// A plain button: identifier, SF Symbol, label, tooltip and what it does.
type Tool = (&'static str, &'static str, &'static str, &'static str, Run);

const EJECT: Tool = (
    "com.apple.finder.EJCT",
    "eject",
    "Eject",
    "Eject disks and unmount servers",
    |this, window, cx| this.eject_selection(window, cx),
);
const NEW_FOLDER: Tool = (
    "com.apple.finder.NFLD",
    "folder.badge.plus",
    "New Folder",
    "Create a new folder in the current location",
    |this, _, cx| this.new_folder(cx),
);
const DELETE: Tool = (
    "com.apple.finder.TRSH",
    "trash",
    "Delete",
    "Move the selected items to the Trash",
    |this, window, cx| this.run_menu(Act::Trash, window, cx),
);
const CONNECT: Tool = (
    "com.apple.finder.CNCT",
    "rectangle.connected.to.line.below",
    "Connect",
    "Connect to a server",
    |this, _, _| this.connect_sheet = Some(ConnectSheet::default()),
);
const INFO: Tool = (
    "com.apple.finder.INFO",
    "info",
    "Get Info",
    "See information about the selected files and folders",
    |this, window, cx| this.run_menu(Act::GetInfo, window, cx),
);
const QUICK_LOOK: Tool = (
    "com.apple.finder.QUIK",
    "eye",
    "Quick Look",
    "Show items with Quick Look or as an instant slideshow",
    |this, window, cx| this.toggle_quick_look(window, cx),
);
const PREVIEW: Tool = (
    "com.apple.finder.PTGL",
    "sidebar.right",
    "Preview",
    "Show or hide the Preview pane",
    |this, _, _| this.show_preview(!this.preview),
);
const ICLOUD: Tool = (
    "com.apple.finder.AddP",
    "person.badge.plus",
    "iCloud",
    "Share the selected items using iCloud Drive",
    |this, window, cx| this.share_selection(window, cx),
);

static CUSTOMIZATION: LazyLock<ToolbarCustomization> = LazyLock::new(|| {
    let defaults: Vec<String> = DEFAULTS.iter().map(|id| id.to_string()).collect();
    let mut initial = crate::prefs::finder().toolbar.clone();
    if initial.is_empty() {
        initial.clone_from(&defaults);
    }
    ToolbarCustomization {
        defaults,
        initial,
        finish: finish_item,
    }
});

#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCDynamicStoreCopyComputerName(store: *const c_void, encoding: *mut u32) -> *mut NSString;
}

static COMPUTER: LazyLock<String> = LazyLock::new(|| {
    let name = unsafe { SCDynamicStoreCopyComputerName(ptr::null(), ptr::null_mut()) };
    unsafe { Retained::from_raw(name) }.map_or_else(String::new, |name| name.to_string())
});

const VIEWS: [(&str, &str, &str); 4] = [
    ("square.grid.2x2", "as Icons", "icon view"),
    ("list.bullet", "as List", "list view"),
    ("rectangle.split.3x1", "as Columns", "column view"),
    ("squares.below.rectangle", "as Gallery", "gallery view"),
];
const SEARCH_W: f32 = 240.;

fn airdrop_glyph() -> Option<Retained<NSImage>> {
    let art = system_template("SidebarAirDrop", px(AIRDROP_ART))?;
    // `RcBlock`, never `StackBlock`: the image re-runs the handler on every appearance change.
    let handler = RcBlock::new(move |_: NSRect| -> Bool {
        let inset = ((AIRDROP_BOX - f64::from(AIRDROP_ART)) / 2.).round();
        art.drawInRect(NSRect::new(
            NSPoint::new(inset, inset),
            NSSize::new(AIRDROP_ART.into(), AIRDROP_ART.into()),
        ));
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(AIRDROP_BOX, AIRDROP_BOX),
        false,
        &handler,
    );
    image.setTemplate(true);
    Some(image)
}

fn group_glyph() -> Retained<NSImage> {
    let handler = RcBlock::new(move |_: NSRect| -> Bool {
        let inset = (GRID_BOX_W - GRID_W) / 2.;
        NSColor::blackColor().setStroke();
        NSColor::blackColor().setFill();
        for y in [GRID_ROWS[0], GRID_ROWS[2]] {
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(inset, y),
                NSSize::new(GRID_W, GRID_RULE_H),
            ));
        }
        for y in [GRID_ROWS[1], GRID_ROWS[3]] {
            for column in 0..3 {
                let x = inset + column as f64 * (GRID_CELL + GRID_GAP);
                let cell = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    NSRect::new(
                        NSPoint::new(x + 0.5, y + 0.5),
                        NSSize::new(GRID_CELL - 1., GRID_CELL - 1.),
                    ),
                    1.,
                    1.,
                );
                cell.setLineWidth(1.);
                cell.stroke();
            }
        }
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(GRID_BOX_W, GRID_H),
        true,
        &handler,
    );
    image.setTemplate(true);
    image
}

thread_local! {
    static GLYPHS: (Option<Retained<NSImage>>, Retained<NSImage>) =
        (airdrop_glyph(), group_glyph());
}

fn finish_item(item: &NSToolbarItem) {
    let id = item.itemIdentifier().to_string();
    GLYPHS.with(|(airdrop, group)| match id.as_str() {
        // Without this AppKit lays the item run out after the window title.
        NAV_ID => item.setNavigational(true),
        AIRDROP_ID => item.setImage(airdrop.as_deref()),
        GROUP_ID => item.setImage(Some(group)),
        _ => {}
    });
    if let Some(menu) = item.downcast_ref::<NSMenuToolbarItem>() {
        // `MenuEntry::disabled` is a no-op while automatic validation is on.
        menu.menu().setAutoenablesItems(false);
        menu.setShowsIndicator(id != ACTION_ID);
    }
}

// Inserted while a job runs rather than kept hidden: AppKit shows hidden items in its palette.
fn place_progress(window: &Window, shown: bool) {
    let Some(toolbar) = chrome_host(window)
        .window()
        .and_then(|window| window.toolbar())
    else {
        return;
    };
    let ids: Vec<String> = toolbar
        .items()
        .iter()
        .map(|item| item.itemIdentifier().to_string())
        .collect();
    match (shown, ids.iter().position(|id| id == JOBS_ID)) {
        (true, None) => {
            let before = ids.iter().position(|id| id == SHARE_ID || id == SEARCH_ID);
            let at = before.unwrap_or(ids.len()) as isize;
            toolbar.insertItemWithItemIdentifier_atIndex(&NSString::from_str(JOBS_ID), at);
        }
        (false, Some(at)) => toolbar.removeItemAtIndex(at as isize),
        _ => {}
    }
}

pub(crate) fn group_menu(sort: Column, grouped: bool) -> Vec<MenuEntry> {
    let mut entries = vec![
        MenuEntry::item("None").checked(!grouped),
        MenuEntry::separator(),
    ];
    for (column, title) in GROUPINGS {
        let row = MenuEntry::item(*title).checked(grouped && *column == Some(sort));
        entries.push(match column {
            Some(_) => row,
            None => row.disabled(),
        });
    }
    entries
}

impl FinderDemo {
    pub(crate) fn install_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.is_none() {
            return;
        }
        let state = self.toolbar_state();
        let (_, _, picked, shareable, cloud, progress, ..) = state;
        let this = cx.entity().downgrade();
        let nav = this.clone();
        let path = this.clone();
        let going = this.clone();
        let view = this.clone();
        let airdrop = this.clone();
        let tag = this.clone();
        let query = this.clone();
        let jobs = this.clone();
        let group = this.clone();
        let sharing = this.clone();
        let opening = this.clone();
        let action = this.clone();
        let tool = |(id, symbol, label, tip, run): Tool, enabled: bool| {
            let this = this.clone();
            ToolbarItem::symbol(id, symbol, label)
                .tool_tip(tip)
                .enabled(enabled)
                .on_click(move |window, cx| {
                    this.update(cx, |this: &mut Self, cx| {
                        run(this, window, cx);
                        cx.notify();
                    })
                    .ok();
                })
        };
        let rows = self.action_rows();
        let acts = Rc::new(RefCell::new(rows.acts));
        let built = acts.clone();
        let mut items = vec![
            ToolbarItem::group(
                NAV_ID,
                "Back/Forward",
                [
                    ToolbarSegment::new("chevron.left", "back").enabled(self.at > 0),
                    ToolbarSegment::new("chevron.right", "forward")
                        .enabled(self.at + 1 < self.history.len()),
                ],
                0,
            )
            .tool_tip("See folders you viewed previously")
            .selection(ToolbarSelection::Momentary)
            .on_select(move |index, window, cx| {
                nav.update(cx, |this: &mut Self, cx| this.step(index == 1, window, cx))
                    .ok();
            }),
            ToolbarItem::menu(PATH_ID, "list.bullet.indent", "Path", self.path_rows())
                .tool_tip("See the location of the current folder")
                .entries_on_open(move |_, cx| {
                    path.upgrade()
                        .map_or_else(Vec::new, |this| this.read(cx).path_rows())
                })
                .on_select(move |index, window, cx| {
                    going
                        .update(cx, |this: &mut Self, cx| {
                            if let Some(id) = this.path_targets().get(index).copied() {
                                this.go(id, window, cx);
                            }
                        })
                        .ok();
                }),
            ToolbarItem::menu(GROUP_ID, "", "Group", group_menu(self.sort, self.grouped))
                .tool_tip("Change the item grouping (hold down Option to change the sort)")
                .on_select(move |index, _, cx| {
                    group
                        .update(cx, |this: &mut Self, cx| this.group_by(index, cx))
                        .ok();
                }),
            ToolbarItem::group(
                VIEW_ID,
                "View",
                VIEWS.iter().map(|(symbol, label, spoken)| {
                    ToolbarSegment::new(*symbol, *label).description(*spoken)
                }),
                self.view,
            )
            .tool_tip("Show items as icons, in a list, in columns, or in a gallery")
            // Automatic, not Expanded: the capsule has to collapse when the search field opens.
            .group_width(ToolbarGroupWidth::Automatic)
            .on_select(move |index, _, cx| {
                view.update(cx, |this: &mut Self, cx| {
                    this.set_view(index);
                    cx.notify();
                })
                .ok();
            }),
            ToolbarItem::menu(ACTION_ID, "ellipsis", "Action", rows.entries)
                .tool_tip("Perform tasks with the selected items")
                .entries_on_open(move |_, cx| {
                    let menu = opening
                        .upgrade()
                        .map(|this| this.read(cx).action_rows())
                        .unwrap_or_default();
                    *built.borrow_mut() = menu.acts;
                    menu.entries
                })
                .on_select(move |index, window, cx| {
                    let act = acts.borrow().get(index).copied().unwrap_or(Act::Nothing);
                    action
                        .update(cx, |this: &mut Self, cx| this.run_menu(act, window, cx))
                        .ok();
                }),
            tool(EJECT, picked),
            ToolbarItem::space(),
            ToolbarItem::flexible_space(),
            tool(NEW_FOLDER, true),
            tool(DELETE, picked),
            tool(CONNECT, true),
            tool(INFO, true),
            ToolbarItem::search(SEARCH_ID, "Search")
                .placeholder("Search")
                .width(px(SEARCH_W))
                .on_change(move |text, _, cx| {
                    let text = text.to_string();
                    query
                        .update(cx, |this: &mut Self, cx| this.search(text, cx))
                        .ok();
                }),
            tool(QUICK_LOOK, picked),
            ToolbarItem::share(SHARE_ID, "Share", Vec::<String>::new())
                .items_on_open(move |_, cx| {
                    sharing
                        .upgrade()
                        .map_or_else(Vec::new, |this| this.read(cx).share_items())
                })
                .tool_tip("Share the selected items")
                .enabled(shareable),
            ToolbarItem::symbol(TAG_ID, "tag", if picked { "Add Tags" } else { "Edit Tags" })
                .tool_tip("Edit Tags")
                .enabled(picked)
                .on_click(move |window, cx| {
                    tag.update(cx, |this: &mut Self, cx| this.edit_tags(window, cx))
                        .ok();
                }),
            tool(PREVIEW, true),
            ToolbarItem::symbol(AIRDROP_ID, "", "AirDrop")
                .tool_tip("Share the selected items using AirDrop")
                .enabled(shareable)
                .on_click(move |_, cx| {
                    airdrop
                        .update(cx, |this: &mut Self, _| this.airdrop_selection())
                        .ok();
                }),
            tool(ICLOUD, cloud),
        ];
        if progress {
            items.push(
                ToolbarItem::symbol(JOBS_ID, "clock.arrow.circlepath", "Progress")
                    .tool_tip("See the progress of file operations")
                    .on_click(move |_, cx| {
                        jobs.update(cx, |this: &mut Self, cx| {
                            this.jobs_open = !this.jobs_open;
                            cx.notify();
                        })
                        .ok();
                    }),
            );
        }
        set_customizable_toolbar(window, cx, ToolbarStyle::Unified, &items, &CUSTOMIZATION);
        place_progress(window, progress);
        if let Some(ns_window) = chrome_host(window).window() {
            // GPUI hides the title whenever the titlebar is transparent, which `window_options` is.
            ns_window.setTitleVisibility(NSWindowTitleVisibility::Visible);
        }
        self.toolbar_for = Some(state);
    }

    /// What the toolbar shows: rebuild it when any of this moves.
    pub(crate) fn toolbar_state(&self) -> ToolbarState {
        let picked = !self.selected.is_empty();
        let (_, shareable, cloud) = picked
            .then(|| crate::menus::chosen_state(self))
            .unwrap_or_default();
        (
            self.at,
            self.history.len(),
            picked,
            shareable,
            cloud,
            self.progress_shown(),
            self.view,
            self.sort,
            self.grouped,
        )
    }

    /// Finder's Path button: this folder, checked, each folder holding it, then the Mac.
    fn path_rows(&self) -> Vec<MenuEntry> {
        let crumbs = self
            .folder()
            .map_or(&[][..], |folder| folder.crumbs.as_slice());
        crumbs
            .iter()
            .rev()
            .copied()
            .chain([COMPUTER.as_str()])
            .enumerate()
            .map(|(at, name)| MenuEntry::item(name).checked(at == 0))
            .collect()
    }

    fn path_targets(&self) -> Vec<&'static str> {
        let crumbs = self.folder().map_or(0, |folder| folder.crumbs.len());
        let mut targets: Vec<&'static str> = Path::new(self.here())
            .ancestors()
            .take(crumbs)
            .map(intern_path)
            .collect();
        targets.push("/");
        targets
    }

    fn action_rows(&self) -> Menu {
        match self
            .cursor_line()
            .filter(|line| self.selected.contains(&line.key))
        {
            Some(line) => crate::menus::item(&self.selected, &line),
            None => crate::menus::background(self),
        }
    }

    fn group_by(&mut self, index: usize, cx: &mut Context<Self>) {
        // Row 0 is "None", row 1 its separator.
        match index.checked_sub(2).map(|at| GROUPINGS.get(at)) {
            None => self.grouped = false,
            Some(Some((Some(column), _))) => {
                self.grouped = true;
                self.resort((*column, self.ascending), cx);
            }
            Some(_) => return,
        }
        cx.notify();
    }

    pub(crate) fn share_items(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .selected
            .snapshot()
            .iter()
            .filter_map(|key| Path::new(key.as_ref()).file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        names.sort_unstable();
        if names.is_empty() {
            vec![self.folder().map_or("Finder", |folder| folder.title).into()]
        } else {
            names
        }
    }
}
