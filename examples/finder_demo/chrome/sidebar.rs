use std::path::{Path, PathBuf};

use finder_fs::ops::Op;
use gpui::{SharedString, Window, prelude::*, px};
use native_gpui::Material;
use native_gpui::sidebar::{
    NativeSidebar, SidebarDrop, SidebarIcon, SidebarItem, SidebarMove, SidebarOptions,
    sidebar_group, sidebar_item,
};

use crate::app::FinderDemo;
use crate::data::{intern, intern_path, server_symbol, volume_roots};
use crate::ops::After;
use crate::prefs;
use crate::tags::finder_color_rgb;

pub(crate) const SIDEBAR_W: f32 = 192.;
pub(crate) const FAVOURITES: &str = "g-favourites";
pub(crate) const LOCATIONS: &str = "g-locations";

pub(crate) enum Glyph {
    Symbol(&'static str),
    Artwork(&'static str),
    Tag(u8),
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

pub(crate) struct Row {
    pub(crate) id: &'static str,
    pub(crate) title: SharedString,
    pub(crate) glyph: Glyph,
    pub(crate) eject: bool,
    pub(crate) drops: bool,
    pub(crate) muted: bool,
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
        drops: false,
        muted: false,
    }
}

impl Row {
    /// A row an item can be filed into. Never stat a path to decide: a row can name a share
    /// that stopped answering.
    fn dropping(mut self) -> Row {
        self.drops = true;
        self
    }
}

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

fn pinned_title(path: &Path) -> String {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    match name.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("app") || ext == "savedSearch" => {
            stem.to_owned()
        }
        _ => name.into_owned(),
    }
}

fn pinned_glyph(path: &Path) -> Glyph {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match name.rsplit_once('.') {
        Some((_, ext)) if ext.eq_ignore_ascii_case("app") => {
            Glyph::Artwork("SidebarApplicationsFolder")
        }
        Some((_, "savedSearch")) => Glyph::Symbol("folder.badge.gearshape"),
        Some(_) if finder_fs::kind::is_package_name(&name) => Glyph::Symbol("questionmark.square"),
        _ => Glyph::Symbol("folder"),
    }
}

fn trash_path() -> Option<PathBuf> {
    finder_fs::volumes::standard(finder_fs::volumes::Standard::Trash)
}

fn listed_row(listed: finder_fs::sidebar::Listed, location: bool) -> Option<Row> {
    use finder_fs::sidebar::Special::*;
    let id = match (&listed.path, listed.special) {
        (Some(path), _) => intern_path(path),
        (None, Some(AirDrop)) => "airdrop",
        (None, Some(Network)) => "/Network",
        _ => return None,
    };
    let glyph = match listed.special {
        Some(ICloudDrive) => Glyph::Symbol("icloud"),
        Some(FileProvider) => Glyph::Symbol("externaldrive.badge.icloud"),
        Some(AirDrop) => Glyph::Artwork("SidebarAirDrop"),
        Some(Network) => Glyph::Symbol("network"),
        Some(Trash) => Glyph::Symbol("trash"),
        _ if listed.network => Glyph::Symbol(server_symbol(
            listed.server.as_deref().unwrap_or(&listed.title),
        )),
        _ if listed.ejectable => Glyph::Symbol("externaldrive"),
        None if location => Glyph::Symbol("internaldrive"),
        _ => glyph_of(Path::new(id)),
    };
    Some(Row {
        eject: listed.ejectable,
        drops: listed.path.is_some(),
        muted: location,
        ..row(id, listed.title, glyph)
    })
}

fn listed_rows(listed: Vec<finder_fs::sidebar::Listed>, location: bool) -> Vec<Row> {
    listed
        .into_iter()
        .filter_map(|listed| listed_row(listed, location))
        .collect()
}

pub(crate) fn groups() -> Vec<Group> {
    let favourites = listed_rows(finder_fs::sidebar::listed_favorites(), false);
    let locations = listed_rows(finder_fs::sidebar::listed_locations(), true);

    let mut tags: Vec<Row> = crate::tags::all()
        .iter()
        .map(|tag| {
            row(
                intern(&format!("tag:{}", tag.name)),
                tag.name.clone(),
                Glyph::Tag(tag.color),
            )
            .dropping()
        })
        .collect();
    tags.push(Row {
        muted: true,
        ..row("tag:", "All Tags…", Glyph::AllTags)
    });

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
        // Finder's own lists decide which of their rows show, and the demo never writes them.
        if !matches!(group.id, FAVOURITES | LOCATIONS) {
            let removed = prefs::removed(group.id);
            group
                .rows
                .retain(|row| !removed.iter().any(|gone| gone == row.id));
        }
        for path in prefs::pinned(group.id).iter().map(PathBuf::from) {
            let id = intern_path(&path);
            let folder = path.is_dir();
            if !(folder || crate::smart::is_saved(id) && path.is_file())
                || group.rows.iter().any(|row| row.id == id)
            {
                continue;
            }
            group.rows.push(Row {
                drops: folder,
                ..row(id, pinned_title(&path), glyph_of(&path))
            });
        }
    }
    groups
}

pub(crate) fn pinned(id: &str) -> bool {
    [FAVOURITES, LOCATIONS]
        .iter()
        .any(|group| prefs::pinned(group).iter().any(|kept| kept == id))
}

pub(crate) fn stored_rows(groups: &[Group]) -> Vec<Vec<&'static str>> {
    groups
        .iter()
        .map(|group| {
            let saved = match group.id {
                FAVOURITES | LOCATIONS => Vec::new(),
                id => prefs::sidebar_rows(id),
            };
            let mut rows: Vec<&'static str> = saved
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
                            .accepts_drops(row.drops)
                            .muted(row.muted),
                    )
                }))
            })
            .collect()
    }

    pub(crate) fn refresh_sidebar(&mut self) {
        self.sidebar_took(groups());
    }

    pub(crate) fn sidebar_took(&mut self, groups: Vec<Group>) {
        self.sidebar_groups = groups;
        for (rows, group) in self.sidebar_rows.iter_mut().zip(&self.sidebar_groups) {
            rows.retain(|id| group.rows.iter().any(|row| row.id == *id));
            for (at, row) in group.rows.iter().enumerate() {
                if !rows.contains(&row.id) {
                    let after = group.rows[..at]
                        .iter()
                        .rev()
                        .find_map(|before| rows.iter().position(|id| *id == before.id));
                    rows.insert(after.map_or(0, |at| at + 1), row.id);
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
        sidebar.on_drop_paths(cx.listener(|this, dropped: &SidebarDrop, window, cx| {
            match dropped.onto.as_deref() {
                Some(onto) if onto.starts_with("tag:") => {
                    this.tag_dropped(&onto["tag:".len()..], &dropped.paths, cx)
                }
                Some(onto) if Some(Path::new(onto)) == trash_path().as_deref() => {
                    this.run_op(Op::Trash(dropped.paths.clone()), After::Nothing, cx)
                }
                // Any other row that reported itself is a folder: only those take drops.
                Some(onto) => {
                    let keys: Vec<SharedString> = dropped
                        .paths
                        .iter()
                        .map(|path| SharedString::from(path.to_string_lossy().into_owned()))
                        .collect();
                    this.drop_into(intern(onto), &keys, window, cx);
                }
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

    fn tag_dropped(&mut self, tag: &str, paths: &[PathBuf], cx: &mut Context<Self>) {
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
            .filter(|path| path.is_dir() || crate::smart::is_saved(&path.to_string_lossy()))
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

    pub(crate) fn add_to_sidebar(&mut self) {
        let paths = self.paths();
        self.pin_favourite(paths);
    }

    pub(crate) fn pin_favourite(&mut self, paths: Vec<PathBuf>) {
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
        let url =
            objc2_foundation::NSURL::fileURLWithPath(&objc2_foundation::NSString::from_str(id));
        if let Err(error) =
            objc2_app_kit::NSWorkspace::sharedWorkspace().unmountAndEjectDeviceAtURL_error(&url)
        {
            self.complain(&error.localizedDescription().to_string());
            return;
        }
        let left = volume_roots().contains(&intern(id));
        let id = id.to_string();
        // Reading every volume's capacity waits on any network volume that stopped answering.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .spawn(async { crate::data::refresh_volumes() })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.refresh_sidebar();
                if left && this.here().starts_with(id.as_str()) {
                    let home = finder_fs::volumes::standard(finder_fs::volumes::Standard::Home);
                    if let Some(home) = home {
                        this.go(intern_path(&home), window, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
