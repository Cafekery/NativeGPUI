//! The unified toolbar, and the two bars that frame the file list.
//!
//! Owns `FinderDemo::install_toolbar` and the item identifiers, the two glyphs no SF Symbol name
//! reaches, the properties that have to be written onto `NSToolbarItem`s after AppKit builds them,
//! the Group and Action menus, and `FinderDemo::path_bar` / `FinderDemo::status_bar`.
//!
//! Deliberately not here: the sidebar, which is a split-view item rather than a toolbar item, and
//! the window chrome, which `main` configures.

use block2::RcBlock;
use gpui::{AnyElement, SharedString, Window, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor, system_color};
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

/// Finder's Sort By / Group By list, read out of its own View menu. `None` for the four Finder
/// offers that this listing has no column for — the row is still drawn, because a menu missing four
/// of eight rows does not look like Finder's.
pub(crate) const GROUPINGS: &[(Option<Column>, &str)] = &[
    (Some(Column::Name), "Name"),
    (Some(Column::Kind), "Kind"),
    (None, "Date Last Opened"),
    (None, "Date Added"),
    (Some(Column::Modified), "Date Modified"),
    (None, "Date Created"),
    (Some(Column::Size), "Size"),
];

/// Finder's toolbar band is 52pt and AppKit owns it: the height comes from the unified toolbar
/// style, and the GPUI tree below it is inset by `SidebarMetrics::top` rather than by a constant.
/// Every capsule in it is a stadium 36pt tall at y 8…44 with a radius of half its height — none of
/// which is written down here, because `NSToolbarItem` draws all of it.
///
/// What is written down are the two glyphs no symbol name reaches. Both are hand-drawn
/// approximations, measured off a window capture rather than guessed, and neither is claimed to be
/// the real artwork.
///
/// AirDrop: three concentric 1pt arcs about a common centre with a 2pt filled dot at it, each arc
/// interrupted by a gap centred on straight down.
pub(crate) const AIRDROP_RADII: [f64; 3] = [8.5, 6.0, 3.5];
pub(crate) const AIRDROP_DOT_R: f64 = 1.0;
/// Half the angular gap at the bottom of each arc, in degrees. The captured arcs run out at 29.5°,
/// 31° and 38.7° from straight down measured outermost first, which a single gap fits to within
/// the antialiased tail a threshold eats.
pub(crate) const AIRDROP_GAP: f64 = 32.0;
/// The image box the arcs are centred in. An `NSToolbarItem`'s width is its image's width plus a
/// constant 24pt of padding — measured, by building the item at 20 and reading back 44 — and
/// Finder's AirDrop item is 45, so the box is 21. The 1.5pt of slack round the 18pt of ink also
/// keeps the outer stroke's antialiasing off the image edge, which would read as a thin arc.
pub(crate) const AIRDROP_BOX: f64 = 21.;
/// The Group item's grid: an 18pt-wide stack of a rule, three 5pt square outlines, a rule, and
/// three more squares, with no rule under the bottom row. `y` is measured down from the ink top,
/// which is what the capture gives. The chevron beside it is `NSMenuToolbarItem`'s own indicator.
pub(crate) const GRID_W: f64 = 18.;
pub(crate) const GRID_H: f64 = 17.5;
pub(crate) const GRID_RULE_H: f64 = 1.5;
pub(crate) const GRID_CELL: f64 = 5.;
pub(crate) const GRID_GAP: f64 = 1.5;
/// Rule, gap, square row, gap, rule — the four y offsets the capture reads at 17.0, 20.0, 26.5
/// and 29.5 against an ink top of 17.0.
pub(crate) const GRID_ROWS: [f64; 4] = [0., 3., 9.5, 12.5];
/// The image box the 18pt grid is centred in. Finder's Group item is 58pt against 54 for an 18pt
/// image, and item width tracks image width one for one, so the box is 22.
pub(crate) const GRID_BOX_W: f64 = 22.;

pub(crate) const BAR_H: f32 = 26.;

/// Toolbar item identifiers. Named because two of them are matched again after `set_toolbar` to
/// write drawn artwork onto the items AppKit built.
pub(crate) const NAV_ID: &str = "ng.finder.nav";
pub(crate) const AIRDROP_ID: &str = "ng.finder.airdrop";
pub(crate) const VIEW_ID: &str = "ng.finder.view";
pub(crate) const GROUP_ID: &str = "ng.finder.group";
pub(crate) const SHARE_ID: &str = "ng.finder.share";
pub(crate) const TAG_ID: &str = "ng.finder.tag";
pub(crate) const ACTION_ID: &str = "ng.finder.action";
pub(crate) const SEARCH_ID: &str = "ng.finder.search";

/// What the view-mode capsule collapses to when the search field takes the room: Finder's popup
/// reads **as List**, so the segment labels are the View menu's own wording rather than the one-word
/// names a segmented control would carry.
pub(crate) const VIEW_LABELS: &[&str] = &["as Icons", "as List", "as Columns", "as Gallery"];
/// The glyph per view mode. Gallery is `squares.below.rectangle`, which beat every other candidate
/// 0.987 to under 0.775 — the obvious `square.grid.3x1.below.line.grid.1x2` did not place.
pub(crate) const VIEW_SYMBOLS: &[&str] = &[
    "square.grid.2x2",
    "list.bullet",
    "rectangle.split.3x1",
    "squares.below.rectangle",
];
/// What the search field widens to on focus. `NSSearchToolbarItem` adds 8pt of item padding to
/// this, and it is the *item* width that drives the reflow: at 242 the item came out 250 against
/// Finder's 248 and every group left of it moved 205pt instead of 203.
pub(crate) const SEARCH_W: f32 = 240.;

// ---------------------------------------------------------------------------------------------
// The two toolbar glyphs that have no symbol name
// ---------------------------------------------------------------------------------------------

/// Finder's AirDrop glyph, hand-drawn. **Not a match, and not claimed as one.**
///
/// `airdrop`, `airdrop.circle` and `airdrop.square` are all in SF Hello's catalogue marked
/// *private*, and `NSImage(systemSymbolName:)` returns nil for every one of them; the closest
/// public name scores 0.75 by normalised cross-correlation, which is a different glyph. So this
/// draws the shape a window capture reads — three concentric arcs about one centre, gapped at the
/// bottom, over a filled dot — and leaves the fidelity claim to the measurement.
///
/// Template, so AppKit tints it exactly as it tints a symbol: enabled, disabled, and inside a
/// pressed capsule are all the system's decisions rather than a colour written down here.
pub(crate) fn airdrop_glyph() -> Retained<NSImage> {
    let box_size = AIRDROP_BOX;
    let handler = RcBlock::new(move |_: NSRect| -> Bool {
        let centre = NSPoint::new(box_size / 2., box_size / 2.);
        NSColor::blackColor().setStroke();
        NSColor::blackColor().setFill();
        for radius in AIRDROP_RADII {
            // One path per arc: appending an arc to a path that already has a current point draws
            // a line to the new arc's start, which would web the three rings together.
            let arc = NSBezierPath::new();
            arc.setLineWidth(1.);
            arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
                centre,
                radius,
                AIRDROP_GAP - 90.,
                270. - AIRDROP_GAP,
            );
            arc.stroke();
        }
        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(centre.x - AIRDROP_DOT_R, centre.y - AIRDROP_DOT_R),
            NSSize::new(AIRDROP_DOT_R * 2., AIRDROP_DOT_R * 2.),
        ))
        .fill();
        Bool::YES
    });
    // `RcBlock`, never `StackBlock`: the image keeps the handler and re-runs it on every appearance
    // change, so a block living on this frame is a use-after-free the first time dark mode flips.
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(box_size, box_size),
        false,
        &handler,
    );
    image.setTemplate(true);
    image
}

/// The Group item's grid glyph, hand-drawn. **Not a match, and not claimed as one.**
///
/// The closest public symbol scores 0.65, far too weak to substitute, so this reproduces the
/// captured geometry: an 18pt rule, a row of three 5pt square outlines, a second rule, a second
/// row, and no rule underneath.
///
/// Drawn in a flipped context so the offsets read top-down, the way the capture measures them.
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
                // Inset by half the line width, so the *outer* edge of the stroke lands on the
                // 5pt cell the capture measures rather than 0.5pt outside it.
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

/// Everything a `ToolbarItem` cannot say, said directly to the items AppKit has just built.
///
/// Four things live here, and each is a property of one `NSToolbarItem` rather than of the item
/// list, so none of them is expressible in a `ToolbarItem` spec:
///
/// **The drawn glyphs.** `ToolbarItem` names its image by SF Symbol, which is the right shape for
/// eleven of Finder's thirteen glyphs and cannot express the other two at all. Rather than give up
/// the real `NSToolbarItem` subclasses — and with them the capsule, the grey selection pill and the
/// search reflow, which is the entire point — those two items are built with a name that resolves to
/// nothing and the artwork is written on afterwards. A failed override therefore shows an empty
/// capsule, which reads as deliberate, instead of a confidently wrong glyph.
///
/// **`navigational`.** Without it AppKit lays the whole item run out *after* the window title,
/// which put back/forward at x 471 against Finder's 196 and left 279pt of empty band in front of
/// it. Marking the group navigational moves it into the leading section ahead of the title, which
/// is exactly Finder's order: back/forward, then the path, then everything else.
///
/// **`showsIndicator`.** Finder's Action item is 41pt — the same width as Share — and carries no
/// chevron; the Group item does. `NSMenuToolbarItem` shows one by default, and it cost 13pt.
///
/// **`autoenablesItems`.** Automatic validation asks the target for `validateMenuItem:`, and where
/// there is none it enables any row whose target merely responds to the action — which every row
/// here does. Leave it on and `MenuEntry::disabled` is a silent no-op, so Finder's greyed
/// *New Folder with Selection* comes up live. Nested menus do not inherit the flag either, but
/// `sync_menu` already turns it off on every submenu it builds.
pub(crate) fn finish_toolbar_items(window: &Window) -> usize {
    let Some(toolbar) = chrome_host(window).window().and_then(|w| w.toolbar()) else {
        return 0;
    };
    let mut done = 0;
    for item in toolbar.items().iter() {
        let id = item.itemIdentifier().to_string();
        match id.as_str() {
            NAV_ID => item.setNavigational(true),
            AIRDROP_ID => {
                item.setImage(Some(&airdrop_glyph()));
                done += 1;
            }
            GROUP_ID => {
                item.setImage(Some(&group_glyph()));
                done += 1;
            }
            _ => {}
        }
        if let Some(menu) = item.downcast_ref::<NSMenuToolbarItem>() {
            menu.menu().setAutoenablesItems(false);
            menu.setShowsIndicator(id == GROUP_ID);
        }
    }
    done
}

/// The Group item's menu: Finder's grouping choices, in the wording and order its own View menu
/// uses. Decorative — the demo does not group — so the checked row is the sort the list is already
/// on, which is the state the menu would be reporting if it worked.
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

/// The `…` (Action) menu. Rows, symbols and separator placement are Finder's shape; the wording is
/// read out of Finder's own File and View menus rather than invented.
///
/// **Its row list is not measured.** The metrics report never opened this menu, so the *order* and
/// the *sectioning* here are reconstructed rather than captured — unlike the toolbar geometry, which
/// is. Nothing in it does anything, which is the intent: this is a shape, not a feature.
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
    /// Finder's whole chrome, in one 52pt band: back/forward, the path title, AirDrop, the view
    /// picker, Group, Share + Tags + Action sharing a capsule, and the search field.
    ///
    /// Every one of them is a real `NSToolbarItem` subclass, which is what buys the glass capsules,
    /// the neutral grey selection pill and the reflow that shoves four groups 203pt left when the
    /// search field opens. None of that is drawn here; describing it in GPUI is what the two-row
    /// version of this demo did, and it is why it did not look like Finder.
    ///
    /// The 8pt gaps Finder shows between its trailing clusters are `ToolbarItem::space()`; the
    /// items *inside* a cluster are adjacent, and macOS 26 merges adjacent plain items into one
    /// capsule on its own — which is where Share, Tags and Action's shared 110pt capsule comes from.
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
                        // `chevron.backward`/`.forward` render identically — they are aliases.
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
                // Where AppKit draws the window title. Finder's ink starts 8pt after the
                // back/forward item and does not move when the string changes, so it is
                // left-aligned in this region rather than centred in it.
                ToolbarItem::flexible_space(),
                // Wired, not decorative: without a handler `NSToolbarItem` validates itself
                // disabled and draws the glyph dimmed, which is not what Finder shows. Finder's
                // AirDrop shares the selection; this listing's AirDrop is a sidebar location, so
                // the item goes there — the same thing clicking that row does.
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
                // Automatic, not Expanded: the capsule has to collapse into a labelled popup when
                // the search field opens, or the 99pt it should give up never appears and AppKit
                // sweeps Share, Tags and Action into an overflow menu instead of shifting them
                // 203pt left. That reflow is the whole reason these are real toolbar items.
                .group_width(ToolbarGroupWidth::Automatic)
                .on_select(move |index, _, cx| {
                    view.update(cx, |this: &mut Self, cx| {
                        this.view = index;
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
        // Two of Finder's glyphs have no symbol name, and a menu row cannot be greyed while
        // automatic validation is on, so both are settled on the built items.
        finish_toolbar_items(window);
        if let Some(ns_window) = chrome_host(window).window() {
            // GPUI hides the title whenever the titlebar is transparent, and `window_options()`
            // always is. Finder puts the folder name in the toolbar, and with `Unified` that is what
            // a visible title *is*, so put it back.
            ns_window.setTitleVisibility(NSWindowTitleVisibility::Visible);
            // Undo `window_options()`'s transparent titlebar, keeping its full-size content view.
            // A transparent titlebar draws no background, so the 52pt band showed the window's
            // backdrop — 67 in dark against Finder's 29 — and, worse, **the capsules did not
            // render at all**: with an opaque backdrop behind a transparent titlebar the fill, the
            // stroke and the band all measured 29, so there was nothing to see. Opaque here, the
            // band is AppKit's own 29 and every capsule comes back. GPUI still draws under the
            // titlebar, which is fine because the tree is inset by `SidebarMetrics::top`.
            ns_window.setTitlebarAppearsTransparent(false);
        }
        self.toolbar = true;
    }

    /// What the share picker is offered. Text, because the listing is invented and there is no file
    /// behind a row to hand over.
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
            let face = if index == 0 && crumbs.len() > 1 {
                F::Volume
            } else {
                F::Folder
            };
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
                    .child(glyph(face, 13.))
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
                            // Quantised in the handler rather than with `snap_to_ticks`: Finder's
                            // zoom slider draws no tick marks, and every distinct size rasterises
                            // its own set of symbols.
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
