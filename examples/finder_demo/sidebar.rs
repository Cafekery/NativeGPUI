use gpui::{SharedString, Window, prelude::*, px};
use native_gpui::Material;
use native_gpui::sidebar::{
    NativeSidebar, SidebarIcon, SidebarItem, SidebarMove, SidebarOptions, sidebar_group,
    sidebar_item,
};

use crate::FinderDemo;

pub(crate) const SIDEBAR_W: f32 = 184.;

pub(crate) enum Glyph {
    Symbol(&'static str),
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

/// A row: folder id, label (empty means the folder's own name), glyph, whether it ejects.
pub(crate) type SidebarRow = (&'static str, &'static str, Glyph, bool);

/// A group: id, header title, and the rows under it.
pub(crate) type SidebarGroup = (&'static str, &'static str, &'static [SidebarRow]);

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
            ("bakery", "Bakery", Glyph::Symbol("externaldrive"), true),
            ("network", "Network", Glyph::Symbol("network"), false),
            ("trash", "Trash", Glyph::Symbol("trash"), false),
        ],
    ),
    ("g-tags", "Tags", &[]),
];

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

    pub(crate) fn sidebar_follow(&self) {
        let Some(sidebar) = &self.sidebar else {
            return;
        };
        let target = self.target();
        let known = self.sidebar_rows.iter().any(|rows| rows.contains(&target));
        sidebar.set_selected(known.then_some(target));
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
                backdrop: Some(Material::Card),
                ..SidebarOptions::default()
            },
        );
        sidebar.on_select(cx.listener(|this, id: &Option<SharedString>, window, cx| {
            let found = id
                .as_ref()
                .and_then(|id| this.store.get_key_value(id.as_ref()))
                .map(|(id, _)| *id);
            if let Some(id) = found {
                this.go(id, window, cx);
            }
            if let Some(sidebar) = &this.sidebar {
                sidebar.focus_content();
            }
            window.focus(&this.focus, cx);
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
        window.focus(&self.focus, cx);
        self.warm_icons(cx);
    }

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
