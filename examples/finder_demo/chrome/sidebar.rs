//! Finder's sidebar: this Mac's Favorites, the volumes it has mounted, and the user's own tags.

use std::path::{Path, PathBuf};

use gpui::{SharedString, Window, prelude::*, px};
use native_gpui::Material;
use native_gpui::sidebar::{
    NativeSidebar, SidebarDrop, SidebarIcon, SidebarItem, SidebarMove, SidebarOptions,
    sidebar_group, sidebar_item,
};

use crate::app::FinderDemo;
use crate::data::{intern, intern_path, volume_roots};
use crate::prefs;
use crate::tags::finder_color_rgb;

pub(crate) const SIDEBAR_W: f32 = 192.;
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

fn place(which: finder_fs::volumes::Standard) -> Option<Row> {
    let path = finder_fs::volumes::standard(which)?;
    let id = intern_path(&path);
    let title = path.file_name()?.to_string_lossy().into_owned();
    Some(row(id, title, glyph_of(&path)))
}

/// The glyph Finder gives a folder it knows by name; a pinned bundle wears the one its kind gets,
/// every other folder a plain one, a custom icon included.
fn glyph_of(path: &Path) -> Glyph {
    use finder_fs::volumes::Standard::*;
    let standard = [
        (Applications, Glyph::Artwork("SidebarApplicationsFolder")),
        (Desktop, Glyph::Symbol("menubar.dock.rectangle")),
        (Documents, Glyph::Symbol("doc")),
        (Downloads, Glyph::Symbol("arrow.down.circle")),
        (Home, Glyph::Symbol("house")),
        (Movies, Glyph::Symbol("film")),
        (Music, Glyph::Artwork("SidebarMusicFolder")),
        (Pictures, Glyph::Symbol("photo")),
    ];
    for (which, glyph) in standard {
        if finder_fs::volumes::standard(which).is_some_and(|it| it == path) {
            return glyph;
        }
    }
    let home = finder_fs::volumes::standard(Home).unwrap_or_default();
    let in_home = |leaf: &str| path == home.join(leaf);
    match path.to_string_lossy().as_ref() {
        "/Applications" => Glyph::Artwork("SidebarApplicationsFolder"),
        "/Applications/Utilities" => Glyph::Symbol("wrench.and.screwdriver.fill"),
        "/Library" | "/System/Library" => Glyph::Symbol("building.classical.columns"),
        "/Users" => Glyph::Symbol("person.crop.circle"),
        _ if in_home("Developer") => Glyph::Symbol("hammer"),
        _ if in_home("Public") => Glyph::Symbol("figure.walk.diamond"),
        _ if in_home("Library") => Glyph::Symbol("building.classical.columns"),
        _ if in_home("Applications") => Glyph::Artwork("SidebarApplicationsFolder"),
        _ => pinned_glyph(path),
    }
}

/// An app is the one bundle Finder names without its extension; every other package keeps it.
fn pinned_title(path: &Path) -> String {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    match name.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("app") => stem.to_owned(),
        _ => name.into_owned(),
    }
}

/// What Finder gives a pinned bundle: an app wears the Applications artwork rather than its own
/// icon, and any other package the square it gives a kind it has no drawing for.
fn pinned_glyph(path: &Path) -> Glyph {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match name.rsplit_once('.') {
        Some((_, ext)) if ext.eq_ignore_ascii_case("app") => {
            Glyph::Artwork("SidebarApplicationsFolder")
        }
        Some(_) if finder_fs::kind::is_package_name(&name) => Glyph::Symbol("questionmark.square"),
        _ => Glyph::Symbol("folder"),
    }
}

fn folder_row(path: &Path, name: Option<&str>) -> Option<Row> {
    let title = match name {
        Some(name) => name.to_owned(),
        None => path.file_name()?.to_string_lossy().into_owned(),
    };
    Some(row(intern_path(path), title, glyph_of(path)))
}

fn volume_row(volume: &finder_fs::volumes::Volume) -> Row {
    let glyph = match (volume.network, volume.ejectable) {
        (true, _) => Glyph::Symbol("externaldrive.connected.to.line.below"),
        (_, true) => Glyph::Symbol("externaldrive"),
        _ => Glyph::Symbol("internaldrive"),
    };
    Row {
        eject: volume.ejectable,
        ..row(intern_path(&volume.path), volume.name.clone(), glyph)
    }
}

fn icloud_drive() -> Option<Row> {
    let drive = finder_fs::volumes::standard(finder_fs::volumes::Standard::Home)?
        .join("Library/Mobile Documents/com~apple~CloudDocs");
    drive
        .is_dir()
        .then(|| row(intern_path(&drive), "iCloud Drive", Glyph::Symbol("icloud")))
}

fn trash_row() -> Option<Row> {
    let trash = finder_fs::volumes::standard(finder_fs::volumes::Standard::Trash)?;
    Some(row(intern_path(&trash), "Trash", Glyph::Symbol("trash")))
}

/// The Favorites Finder keeps for this user, in the user's order.
fn favourites() -> Vec<Row> {
    use finder_fs::volumes::Standard::*;

    let listed = finder_fs::sidebar::favorites().map(|items| {
        items
            .iter()
            .filter(|item| item.visible)
            .filter_map(|item| folder_row(item.path.as_deref()?, item.name.as_deref()))
            .collect::<Vec<Row>>()
    });
    match listed {
        Some(rows) if !rows.is_empty() => rows,
        _ => [Applications, Desktop, Documents, Downloads, Home]
            .into_iter()
            .filter_map(place)
            .collect(),
    }
}

/// Whether Finder is set to show the class of volume this one belongs to.
fn shown(list: &finder_fs::sidebar::Locations, volume: &finder_fs::volumes::Volume) -> bool {
    let names = |item: &&finder_fs::sidebar::Item| {
        item.path.as_deref() == Some(volume.path.as_path())
            || (item.path.is_none() && item.name.as_deref() == Some(volume.name.as_str()))
    };
    if list.items.iter().filter(names).any(|item| !item.visible) {
        return false;
    }
    match (volume.network, volume.ejectable) {
        (true, _) => list.network,
        (_, true) => list.ejectable,
        _ => list.hard_drives,
    }
}

/// Finder's Locations: the rows of its volume list that are here now, Trash last as Finder has it.
fn locations() -> Vec<Row> {
    use finder_fs::sidebar::Special;

    let mounted = finder_fs::volumes::mounted();
    let Some(list) = finder_fs::sidebar::locations() else {
        return default_locations(&mounted);
    };
    let mut rows: Vec<Row> = Vec::new();
    let mut placed: Vec<&Path> = Vec::new();
    let mut trash = None;
    for item in list.items.iter().filter(|item| item.visible) {
        match item.special {
            Some(Special::ICloudDrive) => rows.extend(icloud_drive()),
            Some(Special::Home) => rows.extend(
                finder_fs::volumes::standard(finder_fs::volumes::Standard::Home)
                    .and_then(|home| folder_row(&home, None)),
            ),
            Some(Special::AirDrop) => {
                rows.push(row("airdrop", "AirDrop", Glyph::Artwork("SidebarAirDrop")))
            }
            Some(Special::Network) => {
                rows.push(row("/Network", "Network", Glyph::Symbol("network")))
            }
            Some(Special::Trash) => trash = trash_row(),
            Some(Special::Computer | Special::RemoteDisc) => {}
            None => {
                if let Some(volume) = mounted.iter().find(|volume| {
                    item.path.as_deref() == Some(volume.path.as_path())
                        || (item.path.is_none() && item.name.as_deref() == Some(&volume.name))
                }) {
                    rows.push(volume_row(volume));
                    placed.push(&volume.path);
                }
            }
        }
    }
    for volume in &mounted {
        if !placed.contains(&volume.path.as_path()) && shown(&list, volume) {
            rows.push(volume_row(volume));
        }
    }
    rows.extend(trash);
    rows
}

fn default_locations(mounted: &[finder_fs::volumes::Volume]) -> Vec<Row> {
    let mut rows: Vec<Row> = icloud_drive().into_iter().collect();
    rows.extend(mounted.iter().map(volume_row));
    if Path::new("/Network").is_dir() {
        rows.push(row("/Network", "Network", Glyph::Symbol("network")));
    }
    rows.extend(trash_row());
    rows
}

/// The sidebar as Finder has it for this user: their Favorites, their Locations, their tags.
pub(crate) fn groups() -> Vec<Group> {
    let favourites = favourites();
    let locations = locations();

    let mut tags: Vec<Row> = crate::tags::all()
        .iter()
        .map(|tag| {
            row(
                intern(&format!("tag:{}", tag.name)),
                tag.name.clone(),
                Glyph::Tag(tag.color),
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
        let removed = prefs::removed(group.id);
        group
            .rows
            .retain(|row| !removed.iter().any(|gone| gone == row.id));
        for path in prefs::pinned(group.id).iter().map(PathBuf::from) {
            let id = intern_path(&path);
            if !path.is_dir() || group.rows.iter().any(|row| row.id == id) {
                continue;
            }
            group
                .rows
                .push(row(id, pinned_title(&path), glyph_of(&path)));
        }
    }
    groups
}

/// A row the user dragged in: the only row of a Mac's sidebar Finder lets you rename.
pub(crate) fn pinned(id: &str) -> bool {
    [FAVOURITES, LOCATIONS]
        .iter()
        .any(|group| prefs::pinned(group).iter().any(|kept| kept == id))
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
                min_thickness: px(120.),
                max_thickness: px(0.),
                content_min_thickness: px(crate::preview::LIST_MIN_W),
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
            |id| crate::menus::sidebar(id).entries,
            move |id, index, window, cx| {
                let picked = crate::menus::sidebar(id).acts.get(index).copied();
                let Some(act) = picked else {
                    return;
                };
                demo.update(cx, |this: &mut FinderDemo, cx| {
                    this.run_menu(act, window, cx);
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

    pub(crate) fn remove_from_sidebar(&mut self, id: &str) {
        for group in [FAVOURITES, LOCATIONS] {
            let pinned = prefs::pinned(group);
            let keep: Vec<&str> = pinned
                .iter()
                .map(String::as_str)
                .filter(|kept| *kept != id)
                .collect();
            if keep.len() != pinned.len() {
                prefs::set_pinned(group, &keep);
            }
        }
        if let Some(group) = self
            .sidebar_groups
            .iter()
            .find(|group| group.rows.iter().any(|row| row.id == id))
        {
            let mut removed = prefs::removed(group.id);
            removed.push(id.to_owned());
            let keep: Vec<&str> = removed.iter().map(String::as_str).collect();
            prefs::set_removed(group.id, &keep);
        }
        for rows in &mut self.sidebar_rows {
            rows.retain(|row| *row != id);
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
        let Some(tag) = crate::tags::all().iter().find(|it| it.name == tag) else {
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

    /// Finder's ⌃⌘T: the chosen folders join Favorites, where a drop onto it would put them.
    pub(crate) fn add_to_sidebar(&mut self) {
        let paths: Vec<std::path::PathBuf> = self.paths();
        if paths.is_empty() {
            return;
        }
        let index = self
            .sidebar_groups
            .iter()
            .position(|group| group.id == FAVOURITES)
            .map_or(0, |at| self.sidebar_rows[at].len());
        self.pin(&SidebarDrop {
            paths,
            group: FAVOURITES.into(),
            index,
            onto: None,
        });
    }

    fn store_rows(&self, group: usize) {
        prefs::set_sidebar_rows(self.sidebar_groups[group].id, &self.sidebar_rows[group]);
    }

    pub(crate) fn eject(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = std::path::PathBuf::from(id);
        let url =
            objc2_foundation::NSURL::fileURLWithPath(&objc2_foundation::NSString::from_str(id));
        if let Err(error) =
            objc2_app_kit::NSWorkspace::sharedWorkspace().unmountAndEjectDeviceAtURL_error(&url)
        {
            self.complain(&error.localizedDescription().to_string());
            return;
        }
        let left = volume_roots().contains(&intern_path(&path));
        self.refresh_sidebar();
        if left && self.here().starts_with(id) {
            if let Some(home) = finder_fs::volumes::standard(finder_fs::volumes::Standard::Home) {
                self.go(intern_path(&home), window, cx);
            }
        }
        cx.notify();
    }
}
