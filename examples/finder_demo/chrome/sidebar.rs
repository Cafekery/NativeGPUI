//! Finder's sidebar: this Mac's Favorites, the volumes it has mounted, and the user's own tags.

use std::path::PathBuf;

use gpui::{SharedString, Window, prelude::*, px};
use native_gpui::sidebar::{
    NativeSidebar, SidebarDrop, SidebarIcon, SidebarItem, SidebarMove, SidebarOptions,
    sidebar_group, sidebar_item,
};
use native_gpui::{Material, MenuEntry};

use crate::app::FinderDemo;
use crate::data::{intern, intern_path, volume_roots};
use crate::prefs;
use crate::tags::finder_color_rgb;

pub(crate) const SIDEBAR_W: f32 = 184.;
pub(crate) const FAVOURITES: &str = "g-favourites";
pub(crate) const LOCATIONS: &str = "g-locations";

pub(crate) enum Glyph {
    Symbol(&'static str),
    Artwork(&'static str),
    /// A tag's dot: filled in Finder's own colour for that index, an outline for no colour.
    Tag(u8),
    /// The pair of rings on Finder's "All Tags…" row.
    AllTags,
}

impl Glyph {
    fn icon(&self) -> SidebarIcon {
        match *self {
            Glyph::Symbol(name) => SidebarIcon::Symbol(name.into()),
            Glyph::Artwork(name) => SidebarIcon::SystemResource(name.into()),
            Glyph::Tag(color) => SidebarIcon::Dot(finder_color_rgb(color)),
            Glyph::AllTags => SidebarIcon::Dots,
        }
    }
}

/// A sidebar row: what it opens (a path, or a `tag:` name that opens nothing) and how it looks.
pub(crate) struct Row {
    pub(crate) id: &'static str,
    pub(crate) title: SharedString,
    pub(crate) glyph: Glyph,
    pub(crate) eject: bool,
}

pub(crate) struct Group {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) rows: Vec<Row>,
}

fn row(id: &'static str, title: impl Into<SharedString>, glyph: Glyph) -> Row {
    Row {
        id,
        title: title.into(),
        glyph,
        eject: false,
    }
}

fn place(which: finder_fs::volumes::Standard, glyph: Glyph) -> Option<Row> {
    let path = finder_fs::volumes::standard(which)?;
    let id = intern_path(&path);
    let title = path.file_name()?.to_string_lossy().into_owned();
    Some(row(id, title, glyph))
}

/// The sidebar as this Mac has it: the standard folders, every mounted volume, the user's tags.
pub(crate) fn groups() -> Vec<Group> {
    use finder_fs::volumes::Standard::*;

    let favourites = [
        (Applications, Glyph::Artwork("SidebarApplicationsFolder")),
        (Desktop, Glyph::Symbol("menubar.dock.rectangle")),
        (Documents, Glyph::Symbol("doc")),
        (Downloads, Glyph::Symbol("arrow.down.circle")),
        (Home, Glyph::Symbol("house")),
    ]
    .into_iter()
    .filter_map(|(which, glyph)| place(which, glyph))
    .collect();

    let mut locations = Vec::new();
    if let Some(home) = finder_fs::volumes::standard(Home) {
        let drive = home.join("Library/Mobile Documents/com~apple~CloudDocs");
        if drive.is_dir() {
            locations.push(row(
                intern_path(&drive),
                "iCloud Drive",
                Glyph::Symbol("icloud"),
            ));
        }
    }
    for volume in finder_fs::volumes::mounted() {
        let id = intern_path(&volume.path);
        let glyph = match (volume.network, volume.ejectable) {
            (true, _) => Glyph::Symbol("externaldrive.connected.to.line.below"),
            (_, true) => Glyph::Symbol("externaldrive"),
            _ => Glyph::Symbol("internaldrive"),
        };
        locations.push(Row {
            eject: volume.ejectable,
            ..row(id, volume.name, glyph)
        });
    }
    if std::path::Path::new("/Network").is_dir() {
        locations.push(row("/Network", "Network", Glyph::Symbol("network")));
    }
    if let Some(trash) = finder_fs::volumes::standard(Trash) {
        locations.push(row(intern_path(&trash), "Trash", Glyph::Symbol("trash")));
    }

    let mut tags: Vec<Row> = finder_fs::tags::all()
        .into_iter()
        .map(|tag| {
            let color = tag.color;
            row(
                intern(&format!("tag:{}", tag.name)),
                tag.name,
                Glyph::Tag(color),
            )
        })
        .collect();
    tags.push(row("tag:", "All Tags…", Glyph::AllTags));

    let mut groups = vec![
        Group {
            id: FAVOURITES,
            title: "Favorites",
            rows: favourites,
        },
        Group {
            id: LOCATIONS,
            title: "Locations",
            rows: locations,
        },
        Group {
            id: "g-tags",
            title: "Tags",
            rows: tags,
        },
    ];
    for group in &mut groups {
        for path in prefs::pinned(group.id).iter().map(PathBuf::from) {
            let id = intern_path(&path);
            if !path.is_dir() || group.rows.iter().any(|row| row.id == id) {
                continue;
            }
            let title = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            group
                .rows
                .push(row(id, title.into_owned(), Glyph::Symbol("folder")));
        }
    }
    groups
}

#[derive(Clone, Copy, PartialEq)]
enum Act {
    Nothing,
    Remove,
}

/// Finder's menu on a sidebar tag row: what it opens, what it forgets, and the colour it wears.
fn tag_menu(id: &SharedString) -> Option<(Vec<Act>, Vec<MenuEntry>)> {
    let name = id.strip_prefix("tag:").filter(|name| !name.is_empty())?;
    let mut acts = vec![Act::Nothing; 7];
    acts[2] = Act::Remove;
    acts.extend(std::iter::repeat_n(
        Act::Nothing,
        1 + crate::tags::tags().len(),
    ));
    let entries = [
        MenuEntry::item("Open in New Tab")
            .symbol("arrow.up.forward.app")
            .disabled(),
        MenuEntry::separator(),
        MenuEntry::item("Remove from Sidebar").symbol("xmark"),
        MenuEntry::item(format!("Delete Tag \u{201c}{name}\u{201d}\u{2026}"))
            .symbol("xmark")
            .disabled(),
        MenuEntry::separator(),
        MenuEntry::item(format!("Rename \u{201c}{name}\u{201d}"))
            .symbol("pencil")
            .disabled(),
        MenuEntry::separator(),
    ]
    .into_iter()
    .chain([MenuEntry::item("No Color").disabled()])
    .chain(crate::tags::tags().iter().map(|tag| {
        let entry = MenuEntry::item(tag.name.clone()).disabled();
        match finder_color_rgb(tag.color) {
            Some(color) => entry.dot(color),
            None => entry,
        }
    }))
    .collect();
    Some((acts, entries))
}

/// Finder's menu on a row the user pinned: what it opens, and the pin it forgets.
fn pinned_menu(id: &SharedString) -> Option<(Vec<Act>, Vec<MenuEntry>)> {
    let pinned = [FAVOURITES, LOCATIONS]
        .iter()
        .any(|group| prefs::pinned(group).iter().any(|kept| kept == id.as_ref()));
    if !pinned {
        return None;
    }
    let entries = vec![
        MenuEntry::item("Open in New Tab")
            .symbol("arrow.up.forward.app")
            .disabled(),
        MenuEntry::separator(),
        MenuEntry::item("Remove from Sidebar").symbol("xmark"),
    ];
    Some((vec![Act::Nothing, Act::Nothing, Act::Remove], entries))
}

fn row_menu(id: &SharedString) -> Option<(Vec<Act>, Vec<MenuEntry>)> {
    tag_menu(id).or_else(|| pinned_menu(id))
}

/// Each group's rows in the order the user left them last launch, with anything new after them.
pub(crate) fn stored_rows(groups: &[Group]) -> Vec<Vec<&'static str>> {
    groups
        .iter()
        .map(|group| {
            let mut rows: Vec<&'static str> = prefs::sidebar_rows(group.id)
                .iter()
                .filter_map(|kept| {
                    group
                        .rows
                        .iter()
                        .find(|row| row.id == kept)
                        .map(|row| row.id)
                })
                .collect();
            for row in &group.rows {
                if !rows.contains(&row.id) {
                    rows.push(row.id);
                }
            }
            rows
        })
        .collect()
}

impl FinderDemo {
    pub(crate) fn sidebar_items(&self) -> Vec<SidebarItem> {
        self.sidebar_groups
            .iter()
            .zip(&self.sidebar_rows)
            .map(|(group, order)| {
                sidebar_group(group.id, group.title).children(order.iter().filter_map(|wanted| {
                    let row = group.rows.iter().find(|row| row.id == *wanted)?;
                    Some(
                        sidebar_item(row.id, row.title.clone())
                            .icon(row.glyph.icon())
                            .ejectable(row.eject)
                            .accepts_drops(matches!(row.glyph, Glyph::Tag(_))),
                    )
                }))
            })
            .collect()
    }

    /// Rebuilds around the volumes mounted now, keeping the order the user set.
    pub(crate) fn refresh_sidebar(&mut self) {
        crate::data::refresh_volumes();
        self.sidebar_groups = groups();
        for (rows, group) in self.sidebar_rows.iter_mut().zip(&self.sidebar_groups) {
            rows.retain(|id| group.rows.iter().any(|row| row.id == *id));
            for row in &group.rows {
                if !rows.contains(&row.id) {
                    rows.push(row.id);
                }
            }
        }
        if let Some(sidebar) = &self.sidebar {
            sidebar.update(&self.sidebar_items());
        }
        self.sidebar_follow();
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
            match id.as_deref() {
                Some(path) if path.starts_with('/') => this.go(intern(path), window, cx),
                Some(tag) if tag.starts_with("tag:") => this.show_tag(intern(tag), window, cx),
                _ => {}
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
        sidebar.on_drop_paths(cx.listener(|this, dropped: &SidebarDrop, _, cx| {
            match dropped.onto.as_ref().and_then(|id| id.strip_prefix("tag:")) {
                Some(tag) => this.tag_dropped(tag, &dropped.paths, cx),
                None => this.pin(dropped),
            }
            cx.notify();
        }));
        sidebar.on_eject(cx.listener(|this, id: &SharedString, window, cx| {
            this.eject(id, window, cx);
        }));
        let demo = cx.entity().downgrade();
        sidebar.on_menu(
            |id| row_menu(id).map(|menu| menu.1).unwrap_or_default(),
            move |id, index, _, cx| {
                if row_menu(id).is_none_or(|menu| menu.0.get(index) != Some(&Act::Remove)) {
                    return;
                }
                let id = id.clone();
                demo.update(cx, |this: &mut FinderDemo, cx| {
                    this.remove_from_sidebar(&id);
                    cx.notify();
                })
                .ok();
            },
        );
        sidebar.update(&self.sidebar_items());
        self.sidebar = Some(sidebar);
        self.sidebar_follow();
        self.retitle(window);
        window.focus(&self.focus, cx);
        self.warm_icons();
    }

    fn remove_from_sidebar(&mut self, id: &SharedString) {
        for group in [FAVOURITES, LOCATIONS] {
            let pinned = prefs::pinned(group);
            let keep: Vec<&str> = pinned
                .iter()
                .map(String::as_str)
                .filter(|kept| *kept != id.as_ref())
                .collect();
            if keep.len() != pinned.len() {
                prefs::set_pinned(group, &keep);
            }
        }
        for rows in &mut self.sidebar_rows {
            rows.retain(|row| *row != id.as_ref());
        }
        if let Some(sidebar) = &self.sidebar {
            sidebar.update(&self.sidebar_items());
        }
    }

    fn reorder(&mut self, moved: &SidebarMove) {
        let Some(group) = moved.parent.as_ref().and_then(|parent| {
            self.sidebar_groups
                .iter()
                .position(|group| group.id == parent.as_ref())
        }) else {
            return;
        };
        let rows = &mut self.sidebar_rows[group];
        let Some(from) = rows.iter().position(|id| *id == moved.id.as_ref()) else {
            return;
        };
        let id = rows.remove(from);
        rows.insert(moved.index.min(rows.len()), id);
        self.store_rows(group);
    }

    /// Finder's drop on a tag row: the files keep the tags they had and gain this one.
    fn tag_dropped(&mut self, tag: &str, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let Some(tag) = finder_fs::tags::all().into_iter().find(|it| it.name == tag) else {
            return;
        };
        let favourite = crate::tags::tags()
            .iter()
            .position(|it| it.name == tag.name);
        for path in paths {
            if let Some(index) = favourite {
                let carried = self.tags.entry(intern_path(path).into()).or_default();
                if !carried.contains(&index) {
                    carried.push(index);
                }
            }
            let path = path.clone();
            let tag = tag.clone();
            cx.background_executor()
                .spawn(async move {
                    let mut carried = finder_fs::tags::of(&path);
                    if carried.iter().any(|it| it.name == tag.name) {
                        return;
                    }
                    carried.push(tag);
                    finder_fs::tags::set(&path, &carried).ok();
                })
                .detach();
        }
    }

    fn pin(&mut self, dropped: &SidebarDrop) {
        let group = match dropped.group.as_ref() {
            id @ (FAVOURITES | LOCATIONS) => id,
            _ => return,
        };
        let Some(at) = self.sidebar_groups.iter().position(|it| it.id == group) else {
            return;
        };
        let folders: Vec<&'static str> = dropped
            .paths
            .iter()
            .filter(|path| path.is_dir())
            .map(|path| intern_path(path))
            .collect();
        if folders.is_empty() {
            return;
        }
        let mut pinned = prefs::pinned(self.sidebar_groups[at].id);
        for id in &folders {
            if !pinned.iter().any(|kept| kept == id) {
                pinned.push((*id).to_owned());
            }
        }
        let keep: Vec<&str> = pinned.iter().map(String::as_str).collect();
        prefs::set_pinned(self.sidebar_groups[at].id, &keep);
        self.refresh_sidebar();
        let rows = &mut self.sidebar_rows[at];
        let mut index = dropped.index.min(rows.len());
        for id in folders {
            if let Some(from) = rows.iter().position(|row| *row == id) {
                rows.remove(from);
                index -= usize::from(from < index);
            }
            index = index.min(rows.len());
            rows.insert(index, id);
            index += 1;
        }
        self.store_rows(at);
        if let Some(sidebar) = &self.sidebar {
            sidebar.update(&self.sidebar_items());
        }
    }

    fn store_rows(&self, group: usize) {
        prefs::set_sidebar_rows(self.sidebar_groups[group].id, &self.sidebar_rows[group]);
    }

    fn eject(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let path = std::path::PathBuf::from(id.as_ref());
        let url = objc2_foundation::NSURL::fileURLWithPath(&objc2_foundation::NSString::from_str(
            id.as_ref(),
        ));
        if let Err(error) =
            objc2_app_kit::NSWorkspace::sharedWorkspace().unmountAndEjectDeviceAtURL_error(&url)
        {
            self.complain(&error.localizedDescription().to_string());
            return;
        }
        let left = volume_roots().contains(&intern_path(&path));
        self.refresh_sidebar();
        if left && self.here().starts_with(id.as_ref()) {
            if let Some(home) = finder_fs::volumes::standard(finder_fs::volumes::Standard::Home) {
                self.go(intern_path(&home), window, cx);
            }
        }
        cx.notify();
    }
}
