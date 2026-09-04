use block2::RcBlock;
use gpui::{AnyElement, SharedString, Window, div, prelude::*, px};
use native_gpui::sidebar::system_template;
use native_gpui::tokens::{self, FileType, SystemColor, file_icon, system_color};
use native_gpui::{
    MenuEntry, MenuView, ToolbarGroupWidth, ToolbarItem, ToolbarSegment, ToolbarSelection,
    ToolbarStyle, chrome_host, native_slider, set_toolbar,
};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{NSBezierPath, NSColor, NSImage, NSMenuToolbarItem, NSWindowTitleVisibility};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::data::{AVAILABLE_BYTES, Column, Face as F, byte_string};
use crate::palette::Palette;
use crate::ui::{TAGS, glyph, small_glyph};
use crate::{FinderDemo, GALLERY_VIEW, ICON_VIEW};

pub(crate) const GROUPINGS: &[(Option<Column>, &str)] = &[
    (Some(Column::Name), "Name"),
    (Some(Column::Kind), "Kind"),
    (None, "Date Last Opened"),
    (None, "Date Added"),
    (Some(Column::Modified), "Date Modified"),
    (None, "Date Created"),
    (Some(Column::Size), "Size"),
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

pub(crate) const BAR_H: f32 = 26.;

pub(crate) const NAV_ID: &str = "ng.finder.nav";
pub(crate) const AIRDROP_ID: &str = "ng.finder.airdrop";
pub(crate) const VIEW_ID: &str = "ng.finder.view";
pub(crate) const GROUP_ID: &str = "ng.finder.group";
pub(crate) const SHARE_ID: &str = "ng.finder.share";
pub(crate) const TAG_ID: &str = "ng.finder.tag";
pub(crate) const ACTION_ID: &str = "ng.finder.action";
pub(crate) const SEARCH_ID: &str = "ng.finder.search";

pub(crate) const VIEW_LABELS: &[&str] = &["as Icons", "as List", "as Columns", "as Gallery"];
pub(crate) const VIEW_SYMBOLS: &[&str] = &[
    "square.grid.2x2",
    "list.bullet",
    "rectangle.split.3x1",
    "squares.below.rectangle",
];
pub(crate) const SEARCH_W: f32 = 240.;
const CRUMB_ICON: f32 = 13.;

fn crumb_icon(volume: bool) -> AnyElement {
    let (kind, face) = match volume {
        true => (FileType::Uti("public.volume"), F::Volume),
        false => (FileType::Folder, F::Folder),
    };
    let art = file_icon(kind, px(CRUMB_ICON))
        .map(|symbol| symbol.img().into_any_element())
        .unwrap_or_else(|| glyph(face, CRUMB_ICON));
    div()
        .w(px(CRUMB_ICON))
        .h(px(CRUMB_ICON))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(art)
        .into_any_element()
}

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

/// Properties written onto the `NSToolbarItem`s after AppKit has built them.
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

pub(crate) fn group_menu(sort: Column, ascending: bool) -> Vec<MenuEntry> {
    let mut entries = vec![
        MenuEntry::item("None").checked(true),
        MenuEntry::separator(),
    ];
    for (column, title) in GROUPINGS {
        entries.push(MenuEntry::item(*title).checked(*column == Some(sort)));
    }
    entries.push(MenuEntry::separator());
    entries.push(MenuEntry::item("Ascending").checked(ascending));
    entries.push(MenuEntry::item("Descending").checked(!ascending));
    entries
}

pub(crate) fn action_menu() -> Vec<MenuEntry> {
    let swatches = MenuView::swatches(TAGS.iter().map(|(_, color)| system_color(*color)));
    vec![
        MenuEntry::item("New Folder").symbol("folder.badge.plus"),
        MenuEntry::item("New Folder with Selection")
            .symbol("folder.badge.plus")
            .disabled(),
        MenuEntry::item("Get Info").symbol("info.circle").disabled(),
        MenuEntry::separator(),
        MenuEntry::item("Rename").symbol("pencil"),
        MenuEntry::item("Compress").symbol("archivebox"),
        MenuEntry::item("Duplicate").symbol("plus.square.on.square"),
        MenuEntry::item("Make Alias").symbol("link"),
        MenuEntry::item("Quick Look").symbol("eye"),
        MenuEntry::separator(),
        MenuEntry::item("Copy").symbol("doc.on.doc"),
        MenuEntry::item("Share…").symbol("square.and.arrow.up"),
        MenuEntry::separator(),
        MenuEntry::item("Sort By")
            .symbol("arrow.up.arrow.down")
            .submenu(
                GROUPINGS
                    .iter()
                    .map(|(_, title)| MenuEntry::item(*title))
                    .chain([MenuEntry::separator(), MenuEntry::item("Tags")]),
            ),
        MenuEntry::item("Show View Options").symbol("slider.horizontal.3"),
        MenuEntry::separator(),
        MenuEntry::view(swatches),
        MenuEntry::item("Tags…").symbol("tag"),
        MenuEntry::separator(),
        MenuEntry::item("Import from iPhone or iPad")
            .symbol("iphone")
            .submenu([
                MenuEntry::item("Take Photo").symbol("camera"),
                MenuEntry::item("Scan Documents").symbol("doc.viewfinder"),
                MenuEntry::item("Add Sketch").symbol("pencil.tip.crop.circle"),
            ]),
        MenuEntry::item("Quick Actions").symbol("wand.and.stars"),
    ]
}

impl FinderDemo {
    pub(crate) fn install_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.is_none() {
            return;
        }
        let this = cx.entity().downgrade();
        let nav = this.clone();
        let airdrop = this.clone();
        let view = this.clone();
        let tag = this.clone();
        let query = this.clone();

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
                .selection(ToolbarSelection::Momentary)
                .group_width(ToolbarGroupWidth::Expanded)
                .on_select(move |index, window, cx| {
                    nav.update(cx, |this: &mut Self, cx| this.step(index == 1, window, cx))
                        .ok();
                }),
                ToolbarItem::flexible_space(),
                ToolbarItem::symbol(AIRDROP_ID, "", "AirDrop").on_click(move |window, cx| {
                    airdrop
                        .update(cx, |this: &mut Self, cx| this.go("airdrop", window, cx))
                        .ok();
                }),
                ToolbarItem::group(
                    VIEW_ID,
                    "View",
                    VIEW_SYMBOLS
                        .iter()
                        .zip(VIEW_LABELS)
                        .map(|(symbol, label)| ToolbarSegment::new(*symbol, *label)),
                    self.view,
                )
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
                ToolbarItem::menu(GROUP_ID, "", "Group", group_menu(self.sort, self.ascending)),
                ToolbarItem::space(),
                ToolbarItem::share(SHARE_ID, "Share", self.share_items()),
                ToolbarItem::symbol(TAG_ID, "tag", "Edit Tags").on_click(move |_, cx| {
                    tag.update(cx, |this: &mut Self, cx| {
                        this.cycle_tag();
                        cx.notify();
                    })
                    .ok();
                }),
                ToolbarItem::menu(ACTION_ID, "ellipsis", "Action", action_menu()),
                ToolbarItem::space(),
                ToolbarItem::search(SEARCH_ID, "Search")
                    .placeholder("Search")
                    .width(px(SEARCH_W))
                    .on_change(move |text, _, cx| {
                        let text = text.to_string();
                        query
                            .update(cx, |this: &mut Self, cx| {
                                this.query = text;
                                cx.notify();
                            })
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

    pub(crate) fn path_bar(&self, palette: &Palette) -> AnyElement {
        let crumbs: &[&str] = self.folder().map_or(&[], |folder| folder.crumbs);
        let last = crumbs.len().saturating_sub(1);
        let mut bar = div()
            .h(px(BAR_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(14.))
            .border_t_1()
            .border_color(palette.separator)
            .text_size(tokens::small_system_font_size());

        for (index, crumb) in crumbs.iter().enumerate() {
            if index > 0 {
                if let Some(chevron) = small_glyph("chevron.right", 7., SystemColor::TertiaryLabel)
                {
                    bar = bar.child(chevron);
                }
            }
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .text_color(if index == last {
                        palette.label
                    } else {
                        palette.secondary
                    })
                    .child(crumb_icon(index == 0 && crumbs.len() > 1))
                    .child(*crumb),
            );
        }
        bar.into_any_element()
    }

    pub(crate) fn status_bar(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let count = self.lines(false).len();
        let chosen = self.selected.len();
        let available = byte_string(AVAILABLE_BYTES as u64);
        let text = if chosen > 0 {
            format!("{chosen} of {count} selected, {available} available")
        } else {
            format!("{count} items, {available} available")
        };
        let slider = matches!(self.view, ICON_VIEW | GALLERY_VIEW);

        div()
            .h(px(BAR_H))
            .flex_none()
            .flex()
            .items_center()
            .px(px(12.))
            .border_t_1()
            .border_color(palette.separator)
            .text_size(tokens::small_system_font_size())
            .text_color(palette.secondary)
            .child(div().w(px(120.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .child(SharedString::from(text)),
            )
            .child(
                div()
                    .w(px(120.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .when(slider, |row| {
                        row.child(
                            div().w(px(96.)).child(
                                native_slider("ng.finder.zoom", self.icon_size)
                                    .range(32., 128.)
                                    .on_change(cx.listener(|this, value: &f64, _, cx| {
                                        this.icon_size = (value / 8.).round() * 8.;
                                        cx.notify();
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}
