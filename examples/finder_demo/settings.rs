use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, ElementId, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, WindowBounds, WindowHandle, WindowOptions, div, point, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    CheckState, MenuEntry, ToolbarItem, ToolbarSegment, ToolbarStyle, native_checkbox,
    native_popup, native_root, set_toolbar, window_options,
};
use objc2_foundation::{NSString, NSURL};

use crate::app::FinderDemo;
use crate::chrome::sidebar;
use crate::prefs::{self, WindowTarget};
use crate::tags::finder_color_rgb;

const WIDTH: f32 = 377.;
/// Finder's window height on each pane, title bar and toolbar included.
const HEIGHTS: [f32; 4] = [497., 591., 828., 392.];
const PANES: [(&str, &str); 4] = [
    ("General", "gearshape"),
    ("Tags", "tag"),
    ("Sidebar", "sidebar.left"),
    ("Advanced", "gearshape.2"),
];
const PANE: &str = "ng.finder.settings.pane";
const TOOLBAR: &str = "ng.finder.settings.panes";

const DESKTOP: [(&str, &str, bool); 4] = [
    ("Hard disks", "ShowHardDrivesOnDesktop", false),
    ("External disks", "ShowExternalHardDrivesOnDesktop", true),
    ("CDs, DVDs, and iPods", "ShowRemovableMediaOnDesktop", true),
    ("Connected servers", "ShowMountedServersOnDesktop", false),
];

const ADVANCED: [(&str, &str, bool); 5] = [
    (
        "Show all filename extensions",
        "AppleShowAllExtensions",
        false,
    ),
    (
        "Show warning before changing an extension",
        "FXEnableExtensionChangeWarning",
        true,
    ),
    (
        "Show warning before removing from iCloud Drive",
        "FXEnableRemoveFromICloudDriveWarning",
        false,
    ),
    (
        "Show warning before emptying the Trash",
        "WarnOnEmptyTrash",
        true,
    ),
    (
        "Remove items from the Trash after 30 days",
        "FXRemoveOldTrashItems",
        false,
    ),
];

const FOLDERS_ON_TOP: [(&str, &str); 2] = [
    ("In windows when sorting by name", "_FXSortFoldersFirst"),
    ("On Desktop", "_FXSortFoldersFirstOnDesktop"),
];

const SCOPES: [(&str, &str); 3] = [
    ("Search This Mac", "SCev"),
    ("Search the Current Folder", "SCcf"),
    ("Use the Previous Search Scope", "SCsp"),
];

pub(crate) struct Settings {
    pane: usize,
}

/// One row of the New Finder windows show menu: its title and the `NewWindowTarget` it stores.
struct Place {
    title: String,
    code: Option<&'static str>,
    path: Option<PathBuf>,
}

fn places() -> (Vec<Place>, usize) {
    use finder_fs::volumes::{Standard, standard};
    let home = standard(Standard::Home).unwrap_or_else(|| PathBuf::from("/"));
    let place = |title: String, code| Place {
        title,
        code: Some(code),
        path: None,
    };
    let separator = || Place {
        title: String::new(),
        code: None,
        path: None,
    };
    let name = |path: &Path| finder_fs::name::of_path(path);
    let mut rows = vec![
        place(crate::locations::computer_name().into(), "PfCm"),
        place(name(Path::new("/")), "PfVo"),
        separator(),
        place(name(&home), "PfHm"),
        place("Desktop".into(), "PfDe"),
        place("Documents".into(), "PfDo"),
        place("Recents".into(), "PfAF"),
        place("iCloud Drive".into(), "PfID"),
        separator(),
    ];
    let code = prefs::setting_text("NewWindowTarget");
    let mut chosen = rows.iter().position(|row| row.code == code.as_deref());
    if let (Some("PfLo"), WindowTarget::Folder(path)) =
        (code.as_deref(), prefs::new_window_target())
    {
        chosen = Some(rows.len());
        rows.push(Place {
            title: name(&path),
            code: Some("PfLo"),
            path: Some(path),
        });
        rows.push(separator());
    }
    rows.push(place("Other…".into(), "PfLo"));
    (rows, chosen.unwrap_or(6))
}

fn heading(text: &'static str) -> impl IntoElement {
    div()
        .text_size(tokens::system_font_size())
        .text_color(system_color(SystemColor::Label))
        .child(text)
}

fn toggle(key: &'static str, label: &str, default: bool) -> impl IntoElement {
    native_checkbox(
        ElementId::Name(format!("ng.finder.settings.{key}").into()),
        prefs::setting_on(key, default),
    )
    .title(label)
    .on_change(move |state: &CheckState, _, cx| {
        prefs::set_setting_on(key, *state == CheckState::On);
        applied(key, cx);
    })
}

fn applied(key: &str, cx: &mut App) {
    match key {
        "AppleShowAllExtensions" => {
            apply_names();
            each_window(cx, |this, cx| this.reread_names(cx));
        }
        "_FXSortFoldersFirst" => {
            each_window(cx, |this, cx| this.resort((this.sort, this.ascending), cx));
        }
        _ => {}
    }
    crate::read::notify_windows(cx);
}

fn each_window(cx: &mut App, mut run: impl FnMut(&mut FinderDemo, &mut Context<FinderDemo>)) {
    for window in cx.windows() {
        if let Some(demo) = window.downcast::<FinderDemo>() {
            demo.update(cx, |this, _, cx| run(this, cx)).ok();
        }
    }
}

pub(crate) fn apply_names() {
    finder_fs::name::show_all_extensions(Some(prefs::shows_all_extensions()));
}

impl FinderDemo {
    fn reread_names(&mut self, cx: &mut Context<Self>) {
        let held: Vec<&'static str> = self.folders.held().into_iter().map(|(id, _)| id).collect();
        for id in held {
            if self.folders.shown(id) {
                self.read_folder(id, cx);
            } else {
                self.folders.stale(id);
            }
        }
    }
}

impl Settings {
    fn general(&self, cx: &mut Context<Self>) -> AnyElement {
        let (rows, chosen) = places();
        let entries: Vec<MenuEntry> = rows
            .iter()
            .map(|place| match place.code {
                None => MenuEntry::separator(),
                Some(_) => MenuEntry::item(place.title.clone()),
            })
            .collect();
        let picked = cx.listener(move |_, index: &usize, window, cx| {
            let (places, _) = places();
            let Some(place) = places.get(*index) else {
                return;
            };
            match (place.code, &place.path) {
                (Some("PfLo"), None) => choose_folder(window, cx),
                (Some(code), path) => {
                    prefs::set_setting_text("NewWindowTarget", code);
                    if let Some(path) = path {
                        store_target_path(path);
                    }
                }
                (None, _) => {}
            }
            cx.notify();
        });
        div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(heading("Show these items on the desktop:"))
            .children(
                DESKTOP.iter().map(|(label, key, default)| {
                    div().pl(px(18.)).child(toggle(key, label, *default))
                }),
            )
            .child(div().h(px(14.)))
            .child(heading("New Finder windows show:"))
            .child(
                div().pl(px(18.)).child(
                    native_popup("ng.finder.settings.target", chosen)
                        .entries(entries)
                        .width(px(284.))
                        .on_change(picked),
                ),
            )
            .child(div().h(px(14.)))
            .child(toggle(
                "FinderSpawnTab",
                "Open folders in tabs instead of new windows",
                true,
            ))
            .into_any_element()
    }

    fn tags(&self) -> AnyElement {
        let favourites = crate::tags::tags();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(heading("Show these tags in the sidebar:"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(system_color(SystemColor::Separator))
                    .p(px(4.))
                    .children(crate::tags::all().iter().map(|tag| {
                        let id = crate::data::intern(&format!("tag:{}", tag.name));
                        let state = sidebar::tag_shown(id);
                        div()
                            .flex()
                            .items_center()
                            .h(px(20.))
                            .gap(px(6.))
                            .px(px(6.))
                            .child(tag_dot(tag.color))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(tokens::system_font_size())
                                    .text_color(system_color(SystemColor::Label))
                                    .child(SharedString::from(tag.name.clone())),
                            )
                            .child(
                                native_checkbox(
                                    ElementId::Name(format!("ng.finder.settings.{id}").into()),
                                    false,
                                )
                                .state(state)
                                .on_change(
                                    move |state: &CheckState, _, cx| {
                                        sidebar::show_tag_row(id, *state == CheckState::On);
                                        sidebar::reload_sidebars(cx);
                                    },
                                ),
                            )
                    })),
            )
            .child(div().h(px(8.)))
            .child(
                div()
                    .text_size(tokens::small_system_font_size())
                    .text_color(system_color(SystemColor::SecondaryLabel))
                    .child(
                        "Drag your favorite tags to the area below to make them available for \
                         quick use in Finder menus.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .gap(px(6.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(system_color(SystemColor::Separator))
                    .children(favourites.iter().map(|tag| tag_dot(tag.color))),
            )
            .into_any_element()
    }

    fn sidebar(&self) -> AnyElement {
        let mut pane = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(heading("Show these items in the sidebar:"));
        for (section, rows) in sidebar::shown_sections() {
            if !section.is_empty() {
                pane = pane.child(
                    div()
                        .pt(px(4.))
                        .text_size(tokens::small_system_font_size())
                        .text_color(system_color(SystemColor::SecondaryLabel))
                        .child(section),
                );
            }
            for (shown, label, on) in rows {
                pane = pane.child(
                    div().pl(px(14.)).child(
                        native_checkbox(("ng.finder.settings.row", shown as usize), on)
                            .title(label)
                            .on_change(move |state: &CheckState, _, cx| {
                                sidebar::set_row_shown(shown, *state == CheckState::On);
                                sidebar::reload_sidebars(cx);
                            }),
                    ),
                );
            }
        }
        pane.into_any_element()
    }

    fn advanced(&self) -> AnyElement {
        let scope = prefs::setting_text("FXDefaultSearchScope");
        let scope = SCOPES
            .iter()
            .position(|(_, code)| Some(*code) == scope.as_deref())
            .unwrap_or(0);
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(
                ADVANCED
                    .iter()
                    .map(|(label, key, default)| toggle(key, label, *default)),
            )
            .child(heading("Keep folders on top:"))
            .children(
                FOLDERS_ON_TOP
                    .iter()
                    .map(|(label, key)| div().pl(px(22.)).child(toggle(key, label, false))),
            )
            .child(div().h(px(4.)))
            .child(heading("When performing a search:"))
            .child(
                div().pl(px(22.)).child(
                    native_popup("ng.finder.settings.scope", scope)
                        .items(SCOPES.iter().map(|(label, _)| *label))
                        .width(px(239.))
                        .on_change(|index: &usize, _, _| {
                            prefs::set_setting_text("FXDefaultSearchScope", SCOPES[*index].1);
                        }),
                ),
            )
            .into_any_element()
    }
}

fn tag_dot(color: u8) -> impl IntoElement {
    let dot = div().size(px(12.)).rounded_full();
    match finder_color_rgb(color) {
        Some((r, g, b)) => dot.bg(gpui::rgb(u32::from_be_bytes([0, r, g, b]))),
        None => dot
            .border_1()
            .border_color(system_color(SystemColor::SecondaryLabel)),
    }
}

fn store_target_path(path: &Path) {
    let url =
        NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&path.to_string_lossy()), true);
    if let Some(text) = url.absoluteString() {
        prefs::set_setting_text("NewWindowTargetPath", &text.to_string());
    }
}

fn choose_folder(window: &mut Window, cx: &mut Context<Settings>) {
    let answer = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    cx.spawn_in(window, async move |this, cx| {
        let Ok(Ok(Some(paths))) = answer.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        prefs::set_setting_text("NewWindowTarget", "PfLo");
        store_target_path(&path);
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = match self.pane {
            0 => self.general(cx),
            1 => self.tags(),
            2 => self.sidebar(),
            _ => self.advanced(),
        };
        native_root(
            div()
                .size_full()
                .pt(px(20.))
                .px(px(27.))
                .bg(system_color(SystemColor::WindowBackground))
                .child(pane),
        )
    }
}

fn show_pane(this: &mut Settings, pane: usize, window: &mut Window, cx: &mut Context<Settings>) {
    this.pane = pane;
    prefs::set_setting_text(PANE, PANES[pane].0);
    window.resize(size(px(WIDTH), px(HEIGHTS[pane])));
    cx.notify();
}

pub(crate) fn open(cx: &mut App) {
    cx.defer(show);
}

pub(crate) fn open_pane(pane: &str, cx: &mut App) {
    prefs::set_setting_text(PANE, pane);
    open(cx);
}

fn show(cx: &mut App) {
    let open = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<Settings>());
    if let Some(window) = open {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let remembered = prefs::setting_text(PANE);
    let pane = PANES
        .iter()
        .position(|(name, _)| Some(*name) == remembered.as_deref())
        .unwrap_or(0);
    let opened: Option<WindowHandle<Settings>> = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(969.), px(350.)),
                    size: size(px(WIDTH), px(HEIGHTS[pane])),
                })),
                is_resizable: false,
                is_minimizable: false,
                ..window_options()
            },
            |window, cx| {
                window.set_window_title("Found’er Settings");
                let entity = cx.new(|_| Settings { pane });
                let weak = entity.downgrade();
                let segments = PANES
                    .iter()
                    .map(|(name, symbol)| ToolbarSegment::new(*symbol, *name));
                set_toolbar(
                    window,
                    cx,
                    Some(ToolbarStyle::Unified),
                    &[
                        ToolbarItem::flexible_space(),
                        ToolbarItem::group(TOOLBAR, "Settings", segments, pane).on_select(
                            move |index, window, cx| {
                                weak.update(cx, |this, cx| show_pane(this, index, window, cx))
                                    .ok();
                            },
                        ),
                        ToolbarItem::flexible_space(),
                    ],
                );
                entity
            },
        )
        .ok();
    if let Some(window) = opened {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
    }
}
