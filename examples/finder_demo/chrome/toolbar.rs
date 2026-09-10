use crate::menus::{Act, Menu};
use block2::RcBlock;
use gpui::{Context, Window, px};
use native_gpui::sidebar::system_template;
use native_gpui::{
    MenuEntry, ToolbarGroupWidth, ToolbarItem, ToolbarSegment, ToolbarSelection, ToolbarStyle,
    chrome_host, set_toolbar,
};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{NSBezierPath, NSColor, NSImage, NSMenuToolbarItem, NSWindowTitleVisibility};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::rc::Rc;

use crate::app::FinderDemo;
use crate::data::Column;

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

pub(crate) const AIRDROP_ART: f32 = 18.;
pub(crate) const AIRDROP_BOX: f64 = 21.;
pub(crate) const GRID_W: f64 = 18.;
pub(crate) const GRID_H: f64 = 17.5;
pub(crate) const GRID_RULE_H: f64 = 1.5;
pub(crate) const GRID_CELL: f64 = 5.;
pub(crate) const GRID_GAP: f64 = 1.5;
pub(crate) const GRID_ROWS: [f64; 4] = [0., 3., 9.5, 12.5];
pub(crate) const GRID_BOX_W: f64 = 22.;

pub(crate) const NAV_ID: &str = "ng.finder.nav";
pub(crate) const AIRDROP_ID: &str = "ng.finder.airdrop";
pub(crate) const VIEW_ID: &str = "ng.finder.view";
pub(crate) const GROUP_ID: &str = "ng.finder.group";
pub(crate) const SHARE_ID: &str = "ng.finder.share";
pub(crate) const TAG_ID: &str = "ng.finder.tag";
pub(crate) const ACTION_ID: &str = "ng.finder.action";
pub(crate) const SEARCH_ID: &str = "ng.finder.search";
pub(crate) const JOBS_ID: &str = "ng.finder.jobs";

pub(crate) const VIEW_LABELS: &[&str] = &["as Icons", "as List", "as Columns", "as Gallery"];
pub(crate) const VIEW_SYMBOLS: &[&str] = &[
    "square.grid.2x2",
    "list.bullet",
    "rectangle.split.3x1",
    "squares.below.rectangle",
];
pub(crate) const SEARCH_W: f32 = 240.;
pub(crate) fn airdrop_glyph() -> Option<Retained<NSImage>> {
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

pub(crate) fn group_glyph() -> Retained<NSImage> {
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

pub(crate) fn finish_toolbar_items(window: &Window) {
    let Some(toolbar) = chrome_host(window).window().and_then(|w| w.toolbar()) else {
        return;
    };
    for item in toolbar.items().iter() {
        let id = item.itemIdentifier().to_string();
        match id.as_str() {
            // Without this AppKit lays the item run out after the window title.
            NAV_ID => item.setNavigational(true),
            AIRDROP_ID => {
                if let Some(image) = airdrop_glyph() {
                    item.setImage(Some(&image));
                }
            }
            GROUP_ID => item.setImage(Some(&group_glyph())),
            _ => {}
        }
        if let Some(menu) = item.downcast_ref::<NSMenuToolbarItem>() {
            // `MenuEntry::disabled` is a no-op while automatic validation is on.
            menu.menu().setAutoenablesItems(false);
            menu.setShowsIndicator(id == GROUP_ID);
        }
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
        let this = cx.entity().downgrade();
        let nav = this.clone();
        let view = this.clone();
        let tag = this.clone();
        let query = this.clone();
        let jobs = this.clone();
        let group = this.clone();
        let action = this.clone();
        let rows = self.action_rows();
        let acts: Rc<[Act]> = rows.acts.into();

        set_toolbar(
            window,
            cx,
            Some(ToolbarStyle::Unified),
            &[
                ToolbarItem::group(
                    NAV_ID,
                    "Back/Forward",
                    [
                        ToolbarSegment::new("chevron.left", "Back"),
                        ToolbarSegment::new("chevron.right", "Forward"),
                    ],
                    0,
                )
                .tool_tip("See folders you viewed previously")
                .selection(ToolbarSelection::Momentary)
                .group_width(ToolbarGroupWidth::Expanded)
                .on_select(move |index, window, cx| {
                    nav.update(cx, |this: &mut Self, cx| this.step(index == 1, window, cx))
                        .ok();
                }),
                ToolbarItem::flexible_space(),
                ToolbarItem::symbol(AIRDROP_ID, "", "AirDrop")
                    .tool_tip("Share the selected items using AirDrop"),
                ToolbarItem::group(
                    VIEW_ID,
                    "View",
                    VIEW_SYMBOLS
                        .iter()
                        .zip(VIEW_LABELS)
                        .map(|(symbol, label)| ToolbarSegment::new(*symbol, *label)),
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
                ToolbarItem::space(),
                ToolbarItem::menu(GROUP_ID, "", "Group", group_menu(self.sort, self.grouped))
                    .tool_tip("Change the item grouping (hold down Option to change the sort)")
                    .on_select(move |index, _, cx| {
                        group
                            .update(cx, |this: &mut Self, cx| this.group_by(index, cx))
                            .ok();
                    }),
                ToolbarItem::space(),
                ToolbarItem::symbol(JOBS_ID, "clock.arrow.circlepath", "Progress")
                    .tool_tip("See the progress of file operations")
                    .on_click(move |_, cx| {
                        jobs.update(cx, |this: &mut Self, cx| {
                            this.jobs_open = !this.jobs_open;
                            cx.notify();
                        })
                        .ok();
                    }),
                ToolbarItem::share(SHARE_ID, "Share", self.share_items())
                    .tool_tip("Share the selected items"),
                ToolbarItem::symbol(TAG_ID, "tag", "Edit Tags")
                    .tool_tip("Edit Tags")
                    .on_click(move |_, cx| {
                        tag.update(cx, |this: &mut Self, cx| {
                            this.cycle_tag(cx);
                            cx.notify();
                        })
                        .ok();
                    }),
                ToolbarItem::menu(ACTION_ID, "ellipsis", "Action", rows.entries)
                    .tool_tip("Perform tasks with the selected items")
                    .on_select(move |index, window, cx| {
                        let act = acts.get(index).copied().unwrap_or(Act::Nothing);
                        action
                            .update(cx, |this: &mut Self, cx| this.run_menu(act, window, cx))
                            .ok();
                    }),
                ToolbarItem::space(),
                ToolbarItem::search(SEARCH_ID, "Search")
                    .placeholder("Search")
                    .width(px(SEARCH_W))
                    .on_change(move |text, _, cx| {
                        let text = text.to_string();
                        query
                            .update(cx, |this: &mut Self, cx| this.search(text, cx))
                            .ok();
                    }),
            ],
        );
        finish_toolbar_items(window);
        if let Some(ns_window) = chrome_host(window).window() {
            // GPUI hides the title whenever the titlebar is transparent, which `window_options` is.
            ns_window.setTitleVisibility(NSWindowTitleVisibility::Visible);
        }
        self.toolbar = true;
        self.toolbar_for = (self.selected.rev(), self.cursor.clone());
    }

    /// Finder's Action menu is its context menu for the selection, or for the folder without one.
    fn action_rows(&self) -> Menu {
        match self
            .cursor_line()
            .filter(|line| self.selected.contains(&line.key))
        {
            Some(line) => crate::menus::item(&self.selected, &line),
            None => crate::menus::background(self),
        }
    }

    pub(crate) fn resync_toolbar(&mut self) {
        self.toolbar = false;
    }

    /// A pick in the toolbar's Group menu, which counts its separator: "None" is the row above it.
    fn group_by(&mut self, index: usize, cx: &mut Context<Self>) {
        match index.checked_sub(2).map(|at| GROUPINGS.get(at)) {
            None => self.grouped = false,
            Some(Some((Some(column), _))) => {
                self.grouped = true;
                self.resort((*column, self.ascending), cx);
            }
            Some(_) => return,
        }
        self.resync_toolbar();
        cx.notify();
    }

    pub(crate) fn share_items(&self) -> Vec<String> {
        let names: Vec<String> = self
            .lines(false)
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.row.name.to_string())
            .collect();
        if names.is_empty() {
            vec![self.folder().map_or("Finder", |folder| folder.title).into()]
        } else {
            names
        }
    }
}
