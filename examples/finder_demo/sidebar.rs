//! The native sidebar: its rows, the glyph each one draws, and installing it on the window.
//!
//! Owns `FinderDemo::sidebar_items` and `FinderDemo::install`, plus the sidebar width — chosen for
//! where it leaves the list, not for its own sake.
//!
//! Deliberately not here: the folders the rows open, which are invented content and live in
//! `data`, and the metrics read back out of AppKit, which `main` forwards to the layout.

use gpui::{SharedString, Window, prelude::*, px};
use native_gpui::Material;
use native_gpui::sidebar::{
    NativeSidebar, SidebarIcon, SidebarItem, SidebarMove, SidebarOptions, sidebar_group,
    sidebar_item,
};

use crate::FinderDemo;

/// Chosen for where it leaves the *list*, not for its own width: the sidebar reports an 8pt gutter
/// between its trailing edge and the pane, and Finder's list starts at 192. Measured off a capture
/// — at 192 the pane began at 200 and every column sat 8pt right of Finder's.
pub(crate) const SIDEBAR_W: f32 = 184.;

/// Where a row's glyph comes from. See [`SIDEBAR`] for why a sidebar needs both.
pub(crate) enum Glyph {
    /// An SF Symbol name.
    Symbol(&'static str),
    /// A resource in the system's `CoreTypes.bundle`.
    Artwork(&'static str),
}

impl Glyph {
    fn icon(&self) -> SidebarIcon {
        match *self {
            Glyph::Symbol(name) => SidebarIcon::Symbol(name.into()),
            Glyph::Artwork(name) => SidebarIcon::SystemResource(name.into()),
        }
    }
}

/// A row: its folder id, its label, its glyph, and whether it carries an eject affordance.
///
/// An empty label means the folder's own name, so a row cannot disagree with the path bar.
pub(crate) type SidebarRow = (&'static str, &'static str, Glyph, bool);

/// A group header and the rows under it.
pub(crate) type SidebarGroup = (&'static str, &'static str, &'static [SidebarRow]);

/// The sidebar, as a Mac in front of us has it.
///
/// The glyph route is decided per row by scoring both candidate sets against a capture of the real
/// thing, and the answer is a hybrid rather than a rule. macOS 26's Finder draws **SF Symbols**
/// for most of its own sidebar — `house` 0.975, `trash` 0.974, `hammer` 0.945, `network` 0.864,
/// `doc` 0.802 by normalised cross-correlation — and reaches into its own artwork only where the
/// public catalogue has nothing. Exactly two rows qualify: **AirDrop**, where there is no `airdrop`
/// symbol on this build at all and `SidebarAirDrop` scores 0.892 against the closest public name's
/// 0.597, and **Applications**, 0.961 against `compass.drawing`'s 0.303. Drawing the whole sidebar
/// from the bundle is the tempting simplification and it is wrong in both directions: for
/// Documents the artwork scores 0.076 where `doc` scores 0.802.
///
/// Two rows are close enough that the margin is not evidence: `SidebariCloud` leads `icloud` by
/// 0.041 and `SidebarGenericFolder` trails `folder` by 0.017. Both take the symbol, which is
/// vector artwork that tracks weight and appearance on its own.
pub(crate) const SIDEBAR: &[SidebarGroup] = &[
    (
        "g-favourites",
        "Favorites",
        &[
            (
                "applications",
                "Applications",
                Glyph::Artwork("SidebarApplicationsFolder"),
                false,
            ),
            (
                "desktop",
                "Desktop",
                Glyph::Symbol("menubar.dock.rectangle"),
                false,
            ),
            ("documents", "Documents", Glyph::Symbol("doc"), false),
            ("developer", "Developer", Glyph::Symbol("hammer"), false),
            (
                "downloads",
                "Downloads",
                Glyph::Symbol("arrow.down.circle"),
                false,
            ),
            // The second Downloads is a plain folder somewhere else, not a duplicate of the first
            // — two visibly different glyphs in the capture, and this is the one that reads as
            // `SidebarGenericFolder`/`folder` rather than as the arrow.
            (
                "downloads-bakery",
                "Downloads",
                Glyph::Symbol("folder"),
                false,
            ),
        ],
    ),
    (
        "g-locations",
        "Locations",
        &[
            ("icloud", "iCloud Drive", Glyph::Symbol("icloud"), false),
            ("home", "", Glyph::Symbol("house"), false),
            (
                "airdrop",
                "AirDrop",
                Glyph::Artwork("SidebarAirDrop"),
                false,
            ),
            // The one row nothing identifies: the best of both candidate sets scored 0.19 and the
            // best symbol 0.048, which is a different picture rather than a near miss. A volume
            // carrying its own `.VolumeIcon.icns` would explain that and is not something an
            // invented tree can have, so this glyph is a stand-in and not a match.
            ("bakery", "Bakery", Glyph::Symbol("externaldrive"), true),
            ("network", "Network", Glyph::Symbol("network"), false),
            ("trash", "Trash", Glyph::Symbol("trash"), false),
        ],
    ),
    // Present with nothing under it, which is how a Mac with no tags in use shows it.
    ("g-tags", "Tags", &[]),
];

/// The row order a fresh install has, before the user drags anything.
pub(crate) fn default_rows() -> Vec<Vec<&'static str>> {
    SIDEBAR
        .iter()
        .map(|(_, _, rows)| rows.iter().map(|(id, ..)| *id).collect())
        .collect()
}

impl FinderDemo {
    pub(crate) fn sidebar_items(&self) -> Vec<SidebarItem> {
        SIDEBAR
            .iter()
            .zip(&self.sidebar_rows)
            .map(|((id, title, rows), order)| {
                sidebar_group(*id, *title).children(order.iter().filter_map(|wanted| {
                    let (id, title, glyph, eject) = rows.iter().find(|row| row.0 == *wanted)?;
                    let title = match title.is_empty() {
                        true => self.store.get(id).map_or(*id, |folder| folder.title),
                        false => *title,
                    };
                    Some(
                        sidebar_item(*id, title)
                            .icon(glyph.icon())
                            .ejectable(*eject),
                    )
                }))
            })
            .collect()
    }

    /// Light the row for wherever we are, and nothing when we are somewhere the sidebar has no row
    /// for.
    pub(crate) fn sidebar_follow(&self) {
        let Some(sidebar) = &self.sidebar else {
            return;
        };
        let here = self.here();
        let known = self.sidebar_rows.iter().any(|rows| rows.contains(&here));
        sidebar.set_selected(known.then_some(here));
    }

    pub(crate) fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = NativeSidebar::install(
            window,
            cx,
            SidebarOptions {
                thickness: px(SIDEBAR_W),
                min_thickness: px(150.),
                max_thickness: px(320.),
                reorderable: true,
                // The window's backdrop is what a transparent titlebar shows, so this is the
                // toolbar band's colour as much as the sidebar's. `WindowBackground` measured 67 in
                // dark against Finder's 29, which made the 52pt band the one part of the chrome
                // visibly not the table; `Card` is `ContentBackground`, the opaque one, and lands
                // on the table's own colour.
                backdrop: Some(Material::Card),
                ..SidebarOptions::default()
            },
        );
        // The listing is keyed by `&'static str`, so the row id has to come back out of the map's
        // own keys rather than being cloned out of the notification.
        sidebar.on_select(cx.listener(|this, id: &Option<SharedString>, window, cx| {
            let found = id
                .as_ref()
                .and_then(|id| this.store.get_key_value(id.as_ref()))
                .map(|(id, _)| *id);
            if let Some(id) = found {
                this.go(id, window, cx);
            }
            // Hand the keyboard back to the list, and with it the sidebar's appearance. A source
            // list row is drawn emphasized — accent fill, white label — only while its outline
            // view is the window's first responder, and grey with accent-coloured text otherwise;
            // there is no third "pressed" state to draw, and `NSTableRowView` has no flag for one.
            // Finder looks the way it does because focus lives in the file list, not because it
            // draws its sidebar differently.
            if let Some(sidebar) = &this.sidebar {
                sidebar.focus_content();
            }
            window.focus(&this.focus);
        }));
        sidebar.on_reorder(cx.listener(|this, moved: &SidebarMove, _, cx| {
            this.reorder(moved);
            cx.notify();
        }));
        sidebar.on_eject(cx.listener(|this, id: &SharedString, window, cx| {
            this.eject(id, window, cx);
        }));
        sidebar.update(&self.sidebar_items());
        self.sidebar = Some(sidebar);
        self.sidebar_follow();
        self.retitle(window);
        window.focus(&self.focus);
    }

    /// Apply a drag the outline view has already performed, so the demo's own order is what the
    /// next push writes. Getting this wrong is invisible until something else pushes the model and
    /// the row jumps back.
    fn reorder(&mut self, moved: &SidebarMove) {
        let Some(group) = moved
            .parent
            .as_ref()
            .and_then(|parent| SIDEBAR.iter().position(|(id, ..)| *id == parent.as_ref()))
        else {
            return;
        };
        let rows = &mut self.sidebar_rows[group];
        let Some(from) = rows.iter().position(|id| *id == moved.id.as_ref()) else {
            return;
        };
        let id = rows.remove(from);
        rows.insert(moved.index.min(rows.len()), id);
    }

    /// Ejecting a volume takes its row away, which is the only part of unmounting a demo with no
    /// filesystem behind it can honestly do.
    fn eject(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        for rows in &mut self.sidebar_rows {
            rows.retain(|row| *row != id.as_ref());
        }
        if let Some(sidebar) = &self.sidebar {
            sidebar.update(&self.sidebar_items());
        }
        if self.here() == id.as_ref() {
            self.go("home", window, cx);
        }
        cx.notify();
    }
}
