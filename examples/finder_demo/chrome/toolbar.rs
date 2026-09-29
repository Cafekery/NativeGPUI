use std::cell::RefCell;
use std::iter;
use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;

use block2::RcBlock;
use gpui::{App, Context, WeakEntity, Window, px};
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

use crate::app::FinderDemo;
use crate::data::{Column, GROUP_KEYS, intern_path};
use crate::locations::{COMPUTER, computer_name, is_location, place};
use crate::menus::{Act, Menu};
use crate::view::{GALLERY_VIEW, ICON_VIEW};
use crate::views::header::column_title;
use crate::views::icon::Arrangement;

pub(crate) type ToolbarState = (usize, usize, bool, bool, bool, bool, usize, bool);

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
/// Identifier, SF Symbol, label, tooltip and action.
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
    |_, window, cx| {
        crate::chrome::connect::show(cx.entity().downgrade(), window.window_handle(), cx)
    },
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

fn click(
    this: &WeakEntity<FinderDemo>,
    run: impl Fn(&mut FinderDemo, &mut Window, &mut Context<FinderDemo>) + 'static,
) -> impl Fn(&mut Window, &mut App) + 'static {
    let this = this.clone();
    move |window, cx| {
        this.update(cx, |this, cx| run(this, window, cx)).ok();
    }
}

fn select(
    this: &WeakEntity<FinderDemo>,
    run: impl Fn(&mut FinderDemo, usize, &mut Window, &mut Context<FinderDemo>) + 'static,
) -> impl Fn(usize, &mut Window, &mut App) + 'static {
    let this = this.clone();
    move |index, window, cx| {
        this.update(cx, |this, cx| run(this, index, window, cx))
            .ok();
    }
}

fn read<T: Default>(
    this: &WeakEntity<FinderDemo>,
    build: impl Fn(&FinderDemo) -> T + 'static,
) -> impl Fn(&mut Window, &mut App) -> T + 'static {
    let this = this.clone();
    move |_, cx| {
        this.upgrade()
            .map(|this| build(this.read(cx)))
            .unwrap_or_default()
    }
}

impl FinderDemo {
    pub(crate) fn install_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.is_none() {
            return;
        }
        let state = self.toolbar_state();
        let (_, _, picked, shareable, cloud, progress, _, airdrop) = state;
        let this = cx.entity().downgrade();
        let tool = |(id, symbol, label, tip, run): Tool, enabled: bool| {
            ToolbarItem::symbol(id, symbol, label)
                .tool_tip(tip)
                .enabled(enabled)
                .on_click(click(&this, move |this, window, cx| {
                    run(this, window, cx);
                    cx.notify();
                }))
        };
        let rows = self.action_rows();
        let acts = Rc::new(RefCell::new(rows.acts));
        let built = acts.clone();
        let actions = read(&this, Self::action_rows);
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
            .on_select(select(&this, |this, index, window, cx| {
                this.step(index == 1, window, cx)
            })),
            ToolbarItem::menu(PATH_ID, "list.bullet.indent", "Path", self.path_rows())
                .tool_tip("See the location of the current folder")
                .entries_on_open(read(&this, Self::path_rows))
                .on_select(select(&this, |this, index, window, cx| {
                    if let Some((_, id)) = this.path_places().get(index).copied() {
                        this.go(id, window, cx);
                    }
                })),
            ToolbarItem::menu(GROUP_ID, "", "Group", self.group_menu())
                .tool_tip("Change the item grouping (hold down Option to change the sort)")
                .enabled(!airdrop)
                .entries_on_open(read(&this, Self::group_menu))
                .on_select(select(&this, |this, index, window, cx| {
                    let act = this
                        .group_rows()
                        .get(index)
                        .map_or(Act::Nothing, |row| row.1);
                    this.run_menu(act, window, cx);
                })),
            ToolbarItem::group(
                VIEW_ID,
                "View",
                VIEWS.iter().map(|(symbol, label, spoken)| {
                    ToolbarSegment::new(*symbol, *label).description(*spoken)
                }),
                self.view,
            )
            .tool_tip("Show items as icons, in a list, in columns, or in a gallery")
            .enabled(!airdrop)
            // Automatic, not Expanded: the capsule has to collapse when the search field opens.
            .group_width(ToolbarGroupWidth::Automatic)
            .on_select(select(&this, |this, index, _, cx| {
                this.set_view(index, cx);
                cx.notify();
            })),
            ToolbarItem::menu(ACTION_ID, "ellipsis", "Action", rows.entries)
                .tool_tip("Perform tasks with the selected items")
                .enabled(!airdrop)
                .entries_on_open(move |window, cx| {
                    let menu = actions(window, cx);
                    *built.borrow_mut() = menu.acts;
                    menu.entries
                })
                .on_select(select(&this, move |this, index, window, cx| {
                    let act = acts.borrow().get(index).copied().unwrap_or(Act::Nothing);
                    this.run_menu(act, window, cx);
                })),
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
                .on_change({
                    let this = this.clone();
                    move |text, _, cx| {
                        this.update(cx, |this: &mut Self, cx| this.search(text.to_string(), cx))
                            .ok();
                    }
                }),
            tool(QUICK_LOOK, picked),
            ToolbarItem::share(SHARE_ID, "Share", Vec::<String>::new())
                .items_on_open(read(&this, Self::share_items))
                .tool_tip("Share the selected items")
                .enabled(shareable),
            ToolbarItem::symbol(TAG_ID, "tag", if picked { "Add Tags" } else { "Edit Tags" })
                .tool_tip("Edit Tags")
                .enabled(picked)
                .on_click(click(&this, |this, window, cx| this.edit_tags(window, cx))),
            tool(PREVIEW, true),
            ToolbarItem::symbol(AIRDROP_ID, "", "AirDrop")
                .tool_tip("Share the selected items using AirDrop")
                .enabled(shareable)
                .on_click(click(&this, |this, _, _| this.airdrop_selection())),
            tool(ICLOUD, cloud),
        ];
        if progress {
            items.push(
                ToolbarItem::symbol(JOBS_ID, "clock.arrow.circlepath", "Progress")
                    .tool_tip("See the progress of file operations")
                    .on_click(click(&this, |this, _, cx| {
                        this.jobs_open = !this.jobs_open;
                        cx.notify();
                    })),
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
            self.airdrop_shown(),
        )
    }

    fn path_rows(&self) -> Vec<MenuEntry> {
        self.path_places()
            .into_iter()
            .enumerate()
            .map(|(at, (name, _))| MenuEntry::item(name).checked(at == 0))
            .collect()
    }

    fn path_places(&self) -> Vec<(&'static str, &'static str)> {
        let here = self.here();
        if is_location(here) {
            return iter::successors(Some(here), |id| place(id)?.1)
                .filter_map(|id| Some((place(id)?.0, id)))
                .collect();
        }
        let crumbs = self
            .folder()
            .map_or(&[][..], |folder| folder.crumbs.as_slice());
        let mut places: Vec<(&'static str, &'static str)> = crumbs
            .iter()
            .rev()
            .copied()
            .zip(Path::new(here).ancestors().map(intern_path))
            .collect();
        places.push((computer_name(), COMPUTER));
        places
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

    fn group_menu(&self) -> Vec<MenuEntry> {
        self.group_rows()
            .into_iter()
            .map(|(entry, _)| entry)
            .collect()
    }

    fn group_rows(&self) -> Vec<(MenuEntry, Act)> {
        let here = self.here();
        let group = self.group();
        let arranged = self.arrangement().filter(|_| self.view == ICON_VIEW);
        let sorted = match arranged {
            Some(Arrangement::By(column)) => Some(column),
            Some(_) => None,
            None => Some(self.sort),
        };
        let grouping = |entry: MenuEntry| match self.view == GALLERY_VIEW {
            true => entry.disabled(),
            false => entry,
        };
        let unsorted = MenuEntry::item("None")
            .checked(arranged == Some(Arrangement::Free))
            .option_alternate();
        let mut rows = vec![
            (
                grouping(MenuEntry::item("None").checked(group.is_none())),
                Act::GroupBy(None),
            ),
            match arranged {
                Some(_) => (unsorted, Act::Arrange(Arrangement::Free)),
                None => (unsorted.disabled(), Act::Nothing),
            },
            (MenuEntry::separator(), Act::Nothing),
        ];
        for column in GROUP_KEYS.into_iter().filter(|key| key.offered_at(here)) {
            let title = column_title(column);
            rows.push((
                grouping(MenuEntry::item(title).checked(group == Some(column))),
                Act::GroupBy(Some(column)),
            ));
            let sorts = MenuEntry::item(title)
                .checked(sorted == Some(column))
                .option_alternate();
            rows.push(match column != Column::Application {
                true => (sorts, Act::Sort(column)),
                false => (sorts.disabled(), Act::Nothing),
            });
        }
        rows
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
