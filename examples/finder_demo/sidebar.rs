//! The native sidebar: its rows, and installing it on the window.
//!
//! Owns `FinderDemo::sidebar_items` and `FinderDemo::install`, plus the sidebar width — chosen for
//! where it leaves the list, not for its own sake.
//!
//! Deliberately not here: the `SIDEBAR` table itself, which is invented content and lives in
//! `data`, and the metrics read back out of AppKit, which `main` forwards to the layout.

use gpui::{SharedString, Window, prelude::*, px};
use native_gpui::Material;
use native_gpui::sidebar::{
    NativeSidebar, SidebarItem, SidebarOptions, sidebar_group, sidebar_item,
};

use crate::FinderDemo;
use crate::data::SIDEBAR;

/// Chosen for where it leaves the *list*, not for its own width: the sidebar reports an 8pt gutter
/// between its trailing edge and the pane, and Finder's list starts at 192. Measured off a capture
/// — at 192 the pane began at 200 and every column sat 8pt right of Finder's.
pub(crate) const SIDEBAR_W: f32 = 184.;

impl FinderDemo {
    pub(crate) fn sidebar_items(&self) -> Vec<SidebarItem> {
        SIDEBAR
            .iter()
            .map(|(id, title, rows)| {
                sidebar_group(*id, *title).children(rows.iter().map(
                    |(id, title, symbol, badge)| {
                        let item = sidebar_item(*id, *title).symbol(*symbol);
                        if *badge > 0 { item.badge(*badge) } else { item }
                    },
                ))
            })
            .collect()
    }

    pub(crate) fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = NativeSidebar::install(
            window,
            cx,
            SidebarOptions {
                thickness: px(SIDEBAR_W),
                min_thickness: px(150.),
                max_thickness: px(320.),
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
        }));
        sidebar.update(&self.sidebar_items());
        sidebar.set_selected(Some(self.here()));
        self.sidebar = Some(sidebar);
        self.retitle(window);
        window.focus(&self.focus);
    }
}
