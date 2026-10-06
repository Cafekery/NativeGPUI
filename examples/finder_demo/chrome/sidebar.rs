use std::path::{Path, PathBuf};

use finder_fs::ops::Op;
use finder_fs::sidebar::{Listed, Special};
use finder_fs::volumes::Standard;
use gpui::{App, SharedString, Window, prelude::*, px};
use native_gpui::sidebar::{
    NativeSidebar, SidebarDrop, SidebarIcon, SidebarItem, SidebarMove, SidebarOptions,
    sidebar_group, sidebar_item, system_symbol,
};
use native_gpui::{CheckState, Material};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

use crate::app::FinderDemo;
use crate::data::{intern, intern_path, is_volume_root, server_symbol};
use crate::ops::After;
use crate::prefs;
use crate::tags::tag_system_color;

pub(crate) const SIDEBAR_W: f32 = 216.;
pub(crate) const FAVOURITES: &str = "g-favourites";
pub(crate) const LOCATIONS: &str = "g-locations";

pub(crate) enum Glyph {
    Symbol(&'static str),
    SymbolOr(&'static str, &'static str),
    Tag(u8),
    AllTags,
}

impl Glyph {
    fn icon(&self) -> SidebarIcon {
        match *self {
            Glyph::Symbol(name) => SidebarIcon::Symbol(name.into()),
            Glyph::SymbolOr(name, art) => SidebarIcon::SymbolOrResource(name.into(), art.into()),
            Glyph::Tag(color) => match tag_system_color(color) {
                Some(color) => SidebarIcon::TintedSymbol("circlebadge.fill".into(), color),
                None => SidebarIcon::Symbol("circlebadge".into()),
            },
            Glyph::AllTags => SidebarIcon::Symbol("circlebadge.2".into()),
        }
    }
}

const APPLICATIONS: Glyph = Glyph::SymbolOr("appstore", "SidebarApplicationsFolder");

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
    /// Never stat a path to decide: a row can name a share that stopped answering.
    fn dropping(mut self) -> Row {
        self.drops = true;
        self
    }
}

fn glyph_of(path: &Path) -> Glyph {
    use finder_fs::volumes::Standard::*;
    let standard = [
        (Applications, APPLICATIONS),
        (Desktop, Glyph::Symbol("menubar.dock.rectangle")),
        (Documents, Glyph::Symbol("doc")),
        (Downloads, Glyph::Symbol("arrow.down.circle")),
        (Home, Glyph::Symbol("house")),
        (Movies, Glyph::Symbol("film")),
        (Music, Glyph::SymbolOr("music", "SidebarMusicFolder")),
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
        "/Applications" => APPLICATIONS,
        "/Applications/Utilities" => Glyph::Symbol("wrench.and.screwdriver.fill"),
        "/Library" | "/System/Library" => Glyph::Symbol("building.classical.columns"),
        "/Users" => Glyph::Symbol("person.crop.circle"),
        _ if in_home("Developer") => Glyph::Symbol("hammer"),
        _ if in_home("Public") => Glyph::Symbol("figure.walk.diamond"),
        _ if in_home("Library") => Glyph::Symbol("building.classical.columns"),
        _ if in_home("Applications") => APPLICATIONS,
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
        Some((_, ext)) if ext.eq_ignore_ascii_case("app") => APPLICATIONS,
        Some((_, "savedSearch")) => Glyph::Symbol("folder.badge.gearshape"),
        Some(_) if finder_fs::kind::is_package_name(&name) => Glyph::Symbol("questionmark.square"),
        _ => Glyph::Symbol("folder"),
    }
}

fn trash_path() -> Option<PathBuf> {
    finder_fs::volumes::standard(Standard::Trash)
}

fn provider_glyph(root: Option<&Path>) -> Glyph {
    // A provider may name a symbol from its own bundle, which only it can load.
    let named = root
        .and_then(finder_fs::sidebar::provider_symbol)
        .filter(|name| system_symbol(name).is_some());
    Glyph::Symbol(named.map_or("externaldrive.badge.icloud", |name| intern(&name)))
}

fn listed_row(listed: finder_fs::sidebar::Listed, location: bool) -> Option<Row> {
    use finder_fs::sidebar::Special::*;
    let id = match (&listed.path, listed.special) {
        (Some(path), _) => intern_path(path),
        (None, Some(AirDrop)) => crate::locations::AIRDROP,
        (None, Some(Network)) => crate::locations::NETWORK,
        _ => return None,
    };
    let glyph = match listed.special {
        Some(ICloudDrive) => Glyph::Symbol("icloud"),
        Some(FileProvider) => provider_glyph(listed.path.as_deref()),
        Some(AirDrop) => Glyph::SymbolOr("airdrop", "SidebarAirDrop"),
        Some(Network) => Glyph::Symbol("network"),
        Some(Trash) => Glyph::Symbol("trash"),
        _ if listed.network => Glyph::Symbol(server_symbol(
            listed.server.as_deref().unwrap_or(&listed.title),
        )),
        _ if listed.ejectable => Glyph::Symbol("externaldrive"),
        None if location => Glyph::Symbol("internaldrive"),
        _ => glyph_of(Path::new(id)),
    };
    let title = match listed.special {
        Some(ICloudDrive | AirDrop | Network | Trash) => {
            finder_fs::words::say(&listed.title).into_owned()
        }
        _ => listed.title,
    };
    Some(Row {
        eject: listed.ejectable,
        drops: listed.path.is_some(),
        muted: location,
        ..row(id, title, glyph)
    })
}

fn listed_rows(listed: Vec<finder_fs::sidebar::Listed>, location: bool) -> Vec<Row> {
    listed
        .into_iter()
        .filter_map(|listed| listed_row(listed, location))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Shown {
    Recents,
    Shared,
    Applications,
    Desktop,
    Documents,
    Downloads,
    Movies,
    Music,
    Pictures,
    ICloudDrive,
    CloudStorage,
    Home,
    Computer,
    HardDisks,
    ExternalDisks,
    Removable,
    AirDrop,
    Bonjour,
    Servers,
    Trash,
    RecentTags,
}

const FOLDERS: [(Shown, Standard); 7] = [
    (Shown::Applications, Standard::Applications),
    (Shown::Desktop, Standard::Desktop),
    (Shown::Documents, Standard::Documents),
    (Shown::Downloads, Standard::Downloads),
    (Shown::Movies, Standard::Movies),
    (Shown::Music, Standard::Music),
    (Shown::Pictures, Standard::Pictures),
];

fn key(shown: Shown) -> String {
    format!("ng.finder.sidebar.{shown:?}")
}

/// Finder's own list may name the account's folder rather than the fixture home's.
fn is_standard(path: &Path, which: Standard) -> bool {
    let Some(standard) = finder_fs::volumes::standard(which) else {
        return false;
    };
    path == standard
        || which != Standard::Applications
            && path.file_name() == standard.file_name()
            && path.parent().and_then(Path::parent) == Some(Path::new("/Users"))
}

/// Each row's state as Finder shows it, then as the demo does.
fn visibility(favourites: &[Listed], locations: &[Listed]) -> impl Fn(Shown) -> (bool, bool) {
    let special = |which| locations.iter().any(|listed| listed.special == Some(which));
    let [recents, shared] = finder_fs::sidebar::top_rows_shown();
    let [disks, external, servers] = finder_fs::sidebar::volume_kinds_shown();
    let folders = FOLDERS.iter().map(|&(shown, which)| {
        let listed = favourites.iter().any(|listed| {
            listed
                .path
                .as_deref()
                .is_some_and(|it| is_standard(it, which))
        });
        (shown, listed)
    });
    let recent_tags = prefs::setting_on("ShowRecentTags", true);
    let finder = [
        (Shown::Recents, recents),
        (Shown::Shared, shared),
        (Shown::ICloudDrive, special(Special::ICloudDrive)),
        (Shown::CloudStorage, special(Special::FileProvider)),
        (Shown::Home, special(Special::Home)),
        (Shown::Computer, special(Special::Computer)),
        (Shown::HardDisks, disks),
        (Shown::ExternalDisks, external),
        (Shown::Removable, true),
        (Shown::AirDrop, special(Special::AirDrop)),
        (Shown::Bonjour, true),
        (Shown::Servers, servers),
        (Shown::Trash, special(Special::Trash)),
        (Shown::RecentTags, recent_tags),
    ];
    let all: Vec<(Shown, bool)> = folders.chain(finder).collect();
    move |shown| {
        let finder = all.iter().any(|&(it, on)| it == shown && on);
        (finder, prefs::setting_on(&key(shown), finder))
    }
}

pub(crate) fn shown_sections() -> Vec<(&'static str, Vec<(Shown, String, CheckState)>)> {
    let favourites = finder_fs::sidebar::listed_favorites();
    let locations = finder_fs::sidebar::listed_locations();
    let on = visibility(&favourites, &locations);
    let [disks_mixed, external_mixed] = finder_fs::sidebar::volume_kinds_mixed();
    let row = |shown: Shown, title: String| {
        let mixed = match shown {
            Shown::HardDisks => disks_mixed,
            Shown::ExternalDisks => external_mixed,
            _ => false,
        };
        let state = if mixed && !prefs::has_setting(&key(shown)) {
            CheckState::Mixed
        } else {
            on(shown).1.into()
        };
        (shown, title, state)
    };
    let home = finder_fs::volumes::standard(Standard::Home).unwrap_or_default();
    let computer = crate::locations::computer_name().to_owned();
    let said = |text: &str| finder_fs::words::say(text).into_owned();
    vec![
        (
            "",
            vec![
                row(Shown::Recents, said("Recents")),
                row(Shown::Shared, said("Shared")),
            ],
        ),
        (
            "Favorites",
            FOLDERS
                .iter()
                .map(|&(shown, which)| {
                    let path = finder_fs::volumes::standard(which).unwrap_or_default();
                    row(shown, finder_fs::name::of_path(&path))
                })
                .collect(),
        ),
        (
            "Locations",
            vec![
                row(Shown::ICloudDrive, said("iCloud Drive")),
                row(Shown::CloudStorage, said("Cloud Storage")),
                row(Shown::Home, finder_fs::name::of_path(&home)),
                row(Shown::Computer, computer),
                row(Shown::HardDisks, said("Hard disks")),
                row(Shown::ExternalDisks, said("External disks")),
                row(Shown::Removable, said("CDs, DVDs, and iOS Devices")),
                row(Shown::AirDrop, said("AirDrop")),
                row(Shown::Bonjour, said("Bonjour computers")),
                row(Shown::Servers, said("Connected servers")),
                row(Shown::Trash, said("Trash")),
            ],
        ),
        ("Tags", vec![row(Shown::RecentTags, said("Recent Tags"))]),
    ]
}

pub(crate) fn set_row_shown(shown: Shown, on: bool) {
    prefs::set_setting_on(&key(shown), on);
}

/// A tag row's Settings ▸ Tags checkbox: mixed while it shows only as a recent tag.
pub(crate) fn tag_shown(id: &str) -> CheckState {
    let name = id.strip_prefix("tag:").unwrap_or(id);
    if prefs::removed("g-tags").iter().any(|gone| gone == id) {
        CheckState::Off
    } else if finder_fs::tags::recent()
        .iter()
        .any(|recent| recent == name)
    {
        CheckState::Mixed
    } else {
        CheckState::On
    }
}

pub(crate) fn show_tag_row(id: &str, on: bool) {
    let mut removed = prefs::removed("g-tags");
    removed.retain(|gone| gone != id);
    if !on {
        removed.push(id.to_owned());
    }
    let keep: Vec<&str> = removed.iter().map(String::as_str).collect();
    prefs::set_removed("g-tags", &keep);
}

fn listed(title: String, path: Option<PathBuf>, special: Option<Special>) -> Listed {
    Listed {
        title,
        path,
        special,
        network: false,
        ejectable: false,
        server: None,
    }
}

pub(crate) fn groups() -> Vec<Group> {
    let mut favourites = finder_fs::sidebar::listed_favorites();
    let mut locations = finder_fs::sidebar::listed_locations();
    let on = visibility(&favourites, &locations);
    let shows = |shown| on(shown).1;
    for &(shown, which) in &FOLDERS {
        let here = |listed: &Listed| {
            listed
                .path
                .as_deref()
                .is_some_and(|it| is_standard(it, which))
        };
        match shows(shown) {
            false => favourites.retain(|listed| !here(listed)),
            true if !favourites.iter().any(here) => {
                if let Some(path) = finder_fs::volumes::standard(which) {
                    favourites.push(listed(finder_fs::name::of_path(&path), Some(path), None));
                }
            }
            true => {}
        }
    }
    let wanted = |listed: &Listed| match listed.special {
        Some(Special::Home) => shows(Shown::Home),
        Some(Special::ICloudDrive) => shows(Shown::ICloudDrive),
        Some(Special::FileProvider) => shows(Shown::CloudStorage),
        Some(Special::AirDrop) => shows(Shown::AirDrop),
        Some(Special::Network) => shows(Shown::Bonjour),
        Some(Special::Trash) => shows(Shown::Trash),
        Some(Special::Computer) => shows(Shown::Computer),
        Some(_) => true,
        None if listed.network => shows(Shown::Servers),
        None if listed.ejectable => shows(Shown::ExternalDisks),
        None => shows(Shown::HardDisks),
    };
    let (finder_disks, finder_external, finder_servers) = (
        on(Shown::HardDisks).0,
        on(Shown::ExternalDisks).0,
        on(Shown::Servers).0,
    );
    let hidden_volumes: Vec<Listed> = finder_fs::volumes::mounted()
        .into_iter()
        .filter(|volume| match (volume.network, volume.ejectable) {
            (true, _) => !finder_servers && shows(Shown::Servers),
            (_, true) => !finder_external && shows(Shown::ExternalDisks),
            _ => !finder_disks && shows(Shown::HardDisks),
        })
        .map(|volume| Listed {
            network: volume.network,
            ejectable: volume.ejectable,
            server: volume.server.clone(),
            ..listed(volume.name.clone(), Some(volume.path.clone()), None)
        })
        .collect();
    let before_places = locations
        .iter()
        .position(|listed| matches!(listed.special, Some(Special::AirDrop | Special::Network)))
        .unwrap_or(locations.len());
    locations.splice(before_places..before_places, hidden_volumes);
    let missing =
        |locations: &[Listed], which| !locations.iter().any(|listed| listed.special == Some(which));
    let home = finder_fs::volumes::standard(Standard::Home);
    if shows(Shown::ICloudDrive) && missing(&locations, Special::ICloudDrive) {
        let drive = finder_fs::icloud::drive();
        locations.insert(
            0,
            listed("iCloud Drive".into(), drive, Some(Special::ICloudDrive)),
        );
    }
    if shows(Shown::Home) && missing(&locations, Special::Home) {
        let at = locations
            .iter()
            .position(|listed| {
                !matches!(
                    listed.special,
                    Some(Special::ICloudDrive | Special::FileProvider)
                )
            })
            .unwrap_or(locations.len());
        let title = home
            .as_deref()
            .map(finder_fs::name::of_path)
            .unwrap_or_default();
        locations.insert(at, listed(title, home, Some(Special::Home)));
    }
    if shows(Shown::AirDrop) && missing(&locations, Special::AirDrop) {
        let at = locations
            .iter()
            .position(|listed| listed.network || listed.special == Some(Special::Network))
            .unwrap_or(locations.len());
        locations.insert(at, listed("AirDrop".into(), None, Some(Special::AirDrop)));
    }
    if shows(Shown::Trash) && missing(&locations, Special::Trash) {
        locations.push(listed("Trash".into(), trash_path(), Some(Special::Trash)));
    }
    locations.retain(wanted);
    let mut favourites = listed_rows(favourites, false);
    if shows(Shown::Recents) {
        favourites.insert(
            0,
            row(
                crate::search::RECENTS,
                finder_fs::words::say("Recents").into_owned(),
                Glyph::Symbol("clock"),
            ),
        );
    }
    let locations = listed_rows(locations, true);

    let recent = if shows(Shown::RecentTags) {
        Vec::new()
    } else {
        finder_fs::tags::recent()
    };
    let mut tags: Vec<Row> = crate::tags::all()
        .iter()
        .filter(|tag| !recent.contains(&tag.name))
        .map(|tag| Row {
            muted: tag_system_color(tag.color).is_none(),
            ..row(
                intern(&format!("tag:{}", tag.name)),
                tag.name.clone(),
                Glyph::Tag(tag.color),
            )
            .dropping()
        })
        .collect();
    tags.push(Row {
        muted: true,
        ..row(
            "tag:",
            finder_fs::words::say("All Tags…").into_owned(),
            Glyph::AllTags,
        )
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
        let removed = prefs::removed(group.id);
        group
            .rows
            .retain(|row| !removed.iter().any(|gone| gone == row.id));
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

/// The demo's kept order, each row it has not placed yet after the row it follows in Finder's.
fn arranged(kept: &[String], group: &Group) -> Vec<&'static str> {
    let mut rows: Vec<&'static str> = Vec::new();
    for kept in kept {
        let found = group.rows.iter().find(|row| row.id == kept);
        if let Some(row) = found.filter(|row| !rows.contains(&row.id)) {
            rows.push(row.id);
        }
    }
    for (at, row) in group.rows.iter().enumerate() {
        if !rows.contains(&row.id) {
            let after = group.rows[..at]
                .iter()
                .rev()
                .find_map(|before| rows.iter().position(|id| *id == before.id));
            rows.insert(after.map_or(0, |at| at + 1), row.id);
        }
    }
    rows
}

pub(crate) fn stored_rows(groups: &[Group]) -> Vec<Vec<&'static str>> {
    groups
        .iter()
        .map(|group| arranged(&prefs::sidebar_rows(group.id), group))
        .collect()
}

pub(crate) fn reload_sidebars(cx: &mut App) {
    let windows: Vec<_> = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<FinderDemo>())
        .collect();
    cx.spawn(async move |cx| {
        for window in windows {
            let groups = cx.background_executor().spawn(async { groups() }).await;
            window
                .update(cx, |this, _, cx| {
                    this.sidebar_took(groups);
                    cx.notify();
                })
                .ok();
        }
    })
    .detach();
}

impl FinderDemo {
    pub(crate) fn sidebar_items(&self) -> Vec<SidebarItem> {
        self.sidebar_groups
            .iter()
            .zip(&self.sidebar_rows)
            .map(|(group, order)| {
                sidebar_group(group.id, finder_fs::words::say(group.title).into_owned()).children(
                    order.iter().filter_map(|wanted| {
                        let row = group.rows.iter().find(|row| row.id == *wanted)?;
                        Some(
                            sidebar_item(row.id, row.title.clone())
                                .icon(row.glyph.icon())
                                .ejectable(row.eject)
                                .accepts_drops(row.drops)
                                .muted(row.muted),
                        )
                    }),
                )
            })
            .collect()
    }

    pub(crate) fn refresh_sidebar(&mut self) {
        self.sidebar_took(groups());
    }

    pub(crate) fn sidebar_took(&mut self, groups: Vec<Group>) {
        self.sidebar_rows = stored_rows(&groups);
        self.sidebar_groups = groups;
        self.update_sidebar();
        self.sidebar_follow();
    }

    fn update_sidebar(&self) {
        if let Some(sidebar) = &self.sidebar {
            sidebar.update(&self.sidebar_items());
        }
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
                thickness: px(prefs::finder().sidebar_width.unwrap_or(SIDEBAR_W)),
                min_thickness: px(158.),
                max_thickness: px(0.),
                content_min_thickness: px(crate::preview::LIST_MIN_W),
                reorderable: true,
                backdrop: Some(Material::Card),
                edge_rule: true,
                ..SidebarOptions::default()
            },
        );
        sidebar.on_select(cx.listener(|this, id: &Option<SharedString>, window, cx| {
            match id.as_deref() {
                Some(path) if path.starts_with('/') || crate::locations::is_location(path) => {
                    this.go(intern(path), window, cx)
                }
                Some(tag) if tag.starts_with("tag:") => this.show_tag(intern(tag), window, cx),
                Some(crate::search::RECENTS) => this.show_tag(crate::search::RECENTS, window, cx),
                _ => {}
            }
            if let Some(sidebar) = &this.sidebar {
                sidebar.focus_content();
            }
            window.focus(&this.focus, cx);
        }));
        sidebar.on_reorder(cx.listener(|this, moved: &SidebarMove, _, cx| {
            this.reorder(moved);
            reload_sidebars(cx);
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
                None => this.pin(dropped, cx),
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

    pub(crate) fn remove_from_sidebar(&mut self, id: &str, cx: &mut Context<Self>) {
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
        self.update_sidebar();
        reload_sidebars(cx);
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
        let Some(tag) = crate::tags::all().iter().find(|it| it.name == tag).cloned() else {
            return;
        };
        let add = [tag];
        crate::tags::retag(
            paths.to_vec(),
            move |paths| {
                for path in paths {
                    finder_fs::tags::apply(std::slice::from_ref(path), &add, &[]).ok();
                }
            },
            cx,
        );
    }

    fn pin(&mut self, dropped: &SidebarDrop, cx: &mut Context<Self>) {
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
        let group = self.sidebar_groups[at].id;
        let mut pinned = prefs::pinned(group);
        for id in &folders {
            if !pinned.iter().any(|kept| kept == id) {
                pinned.push((*id).to_owned());
            }
        }
        let keep: Vec<&str> = pinned.iter().map(String::as_str).collect();
        prefs::set_pinned(group, &keep);
        let removed = prefs::removed(group);
        let back: Vec<&str> = removed
            .iter()
            .map(String::as_str)
            .filter(|gone| !folders.iter().any(|id| id == gone))
            .collect();
        if back.len() != removed.len() {
            prefs::set_removed(group, &back);
        }
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
        self.update_sidebar();
        reload_sidebars(cx);
    }

    pub(crate) fn add_to_sidebar(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths();
        self.pin_favourite(paths, cx);
    }

    pub(crate) fn pin_favourite(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let index = self
            .sidebar_groups
            .iter()
            .position(|group| group.id == FAVOURITES)
            .map_or(0, |at| self.sidebar_rows[at].len());
        self.pin(
            &SidebarDrop {
                paths,
                group: FAVOURITES.into(),
                index,
                onto: None,
            },
            cx,
        );
    }

    fn store_rows(&self, group: usize) {
        prefs::set_sidebar_rows(self.sidebar_groups[group].id, &self.sidebar_rows[group]);
    }

    pub(crate) fn eject(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(id));
        if let Err(error) = NSWorkspace::sharedWorkspace().unmountAndEjectDeviceAtURL_error(&url) {
            self.complain(&error.localizedDescription().to_string());
            return;
        }
        let left = is_volume_root(id);
        let id = id.to_string();
        // Reading every volume's capacity waits on any network volume that stopped answering.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .spawn(async { crate::data::refresh_volumes() })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.refresh_sidebar();
                if left && this.here().starts_with(id.as_str()) {
                    if let Some(home) = finder_fs::volumes::standard(Standard::Home) {
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
