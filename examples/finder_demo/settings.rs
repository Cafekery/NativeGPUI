use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, ElementId, FontWeight, IntoElement,
    ParentElement, Render, Styled, TitlebarOptions, UniformListScrollHandle, WeakEntity, Window,
    WindowBounds, WindowHandle, WindowOptions, div, point, px, size,
};
use native_gpui::tokens::{self, FileType, SystemColor};
use native_gpui::{
    CheckState, ControlSize, MenuEntry, NativeLabel, RawMaterial, ToolbarItem, ToolbarStyle,
    material, native_checkbox, native_image, native_label, native_popup, native_root,
    native_symbol, set_toolbar, system_template, window_options,
};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSImage, NSWindow};
use objc2_foundation::{
    NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLIsUbiquitousItemKey, NSUserDefaults,
};

use crate::app::FinderDemo;
use crate::chrome::sidebar::{self, Shown};
use crate::prefs::{self, WindowTarget};

mod tags_pane;

const WIDTH: f32 = 377.;
/// The frames below are Finder's, from its window's top: the content starts under the toolbar.
const CHROME: f32 = 88.;
const CONTENT: [f32; 4] = [407., 503., 740., 304.];
const PANES: [(&str, &str); 4] = [
    ("General", "gearshape"),
    ("Tags", "tag"),
    ("Sidebar", "sidebar.left"),
    ("Advanced", "gearshape.2"),
];
const PANE: &str = "ng.finder.settings.pane";
const ICLOUD_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.systempreferences.AppleIDSettings?iCloud";
const RECENTS_ICON: &str = "/System/Library/CoreServices/Finder.app/Contents/Resources/MyLibraries/myDocuments.cannedSearch";

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
    tag_picked: Option<String>,
    /// The name being typed for a tag just added.
    tag_draft: Option<String>,
    tag_fresh: bool,
    tag_scroll: UniformListScrollHandle,
}

enum Icon {
    Path(PathBuf),
    Art(FileType<'static>),
}

/// One row of the New Finder windows show menu: its title and the `NewWindowTarget` it stores.
struct Place {
    title: String,
    icon: Option<Icon>,
    code: Option<&'static str>,
    path: Option<PathBuf>,
}

impl Place {
    fn entry(&self) -> MenuEntry {
        let entry = MenuEntry::item(self.title.clone());
        match &self.icon {
            Some(Icon::Path(path)) => entry.file_icon(FileType::Path(&path.to_string_lossy())),
            Some(Icon::Art(art)) => entry.file_icon(*art),
            None => entry,
        }
    }
}

fn places() -> (Vec<Place>, usize) {
    use finder_fs::volumes::{Standard, standard};
    let home = standard(Standard::Home).unwrap_or_else(|| PathBuf::from("/"));
    let place = |title: String, icon, code| Place {
        title,
        icon,
        code: Some(code),
        path: None,
    };
    let separator = || Place {
        title: String::new(),
        icon: None,
        code: None,
        path: None,
    };
    let name = |path: &Path| finder_fs::name::of_path(path);
    let folder = |which| Some(Icon::Path(standard(which).unwrap_or_else(|| home.clone())));
    let computer = crate::locations::artwork(crate::locations::COMPUTER).map(Icon::Art);
    let mut rows = vec![
        place(crate::locations::computer_name().into(), computer, "PfCm"),
        place(name(Path::new("/")), Some(Icon::Path("/".into())), "PfVo"),
        separator(),
        place(name(&home), folder(Standard::Home), "PfHm"),
        place("Desktop".into(), folder(Standard::Desktop), "PfDe"),
        place("Documents".into(), folder(Standard::Documents), "PfDo"),
        place(
            "Recents".into(),
            Some(Icon::Path(RECENTS_ICON.into())),
            "PfAF",
        ),
        place(
            "iCloud Drive".into(),
            Some(Icon::Art(crate::chrome::path_bar::icloud_art())),
            "PfID",
        ),
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
            icon: Some(Icon::Path(path.clone())),
            code: Some("PfLo"),
            path: Some(path),
        });
        rows.push(separator());
    }
    rows.push(place("Other…".into(), None, "PfLo"));
    (rows, chosen.unwrap_or(6))
}

fn at(x: f32, y: f32) -> Div {
    div().absolute().left(px(x)).top(px(y - CHROME))
}

fn label(text: &str) -> NativeLabel {
    native_label(
        ElementId::Name(format!("ng.finder.settings.{text}").into()),
        text,
    )
}

fn footnote(text: &str) -> NativeLabel {
    label(text)
        .font_size(px(10.))
        .color(SystemColor::SecondaryLabel)
}

fn lines(x: f32, y: f32, pitch: f32, texts: &[&str], make: fn(&str) -> NativeLabel) -> Div {
    texts.iter().zip(0..).fold(div(), |lines, (text, line)| {
        lines.child(
            at(x, y + pitch * line as f32)
                .w(px(WIDTH - x))
                .child(make(text)),
        )
    })
}

fn global_on(key: &str) -> bool {
    NSUserDefaults::standardUserDefaults().boolForKey(&NSString::from_str(key))
}

fn syncs_desktop() -> bool {
    let Some(desktop) = finder_fs::volumes::standard(finder_fs::volumes::Standard::Desktop) else {
        return false;
    };
    let url =
        NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&desktop.to_string_lossy()), true);
    let mut synced = None;
    let _ = unsafe { url.getResourceValue_forKey_error(&mut synced, NSURLIsUbiquitousItemKey) };
    synced
        .and_then(|synced| synced.downcast::<NSNumber>().ok())
        .is_some_and(|synced| synced.boolValue())
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
                Some(_) => place.entry(),
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
            .child(
                at(28., 104.)
                    .w(px(300.))
                    .child(label("Show these items on the desktop:")),
            )
            .children(DESKTOP.iter().zip(0..).map(|((label, key, default), row)| {
                at(48., 128. + 22. * row as f32).child(toggle(key, label, *default))
            }))
            .child(
                at(28., 234.)
                    .w(px(300.))
                    .child(label("New Finder windows show:")),
            )
            .child(
                at(48., 258.).child(
                    native_popup("ng.finder.settings.target", chosen)
                        .entries(entries)
                        .width(px(284.))
                        .on_change(picked),
                ),
            )
            .child(
                at(30., 311.).child(
                    native_checkbox("ng.finder.settings.sync", syncs_desktop())
                        .title("Sync Desktop & Documents folders")
                        .on_change(|_, window, cx| {
                            cx.open_url(ICLOUD_SETTINGS);
                            window.refresh();
                        }),
                ),
            )
            .child(lines(
                50.5,
                340.,
                13.,
                &[
                    "Store your Desktop & Documents folders in iCloud Drive",
                    "and access them from your other devices.",
                ],
                footnote,
            ))
            .child(at(30., 387.).child(toggle(
                "FinderSpawnTab",
                "Open folders in tabs instead of new windows",
                true,
            )))
            .child(
                at(30., 422.).child(
                    native_checkbox(
                        "ng.finder.settings.names",
                        !global_on("NSSmartNamingDisabled"),
                    )
                    .title("Suggest file names")
                    .enabled(false),
                ),
            )
            .child(lines(
                48.5,
                440.,
                13.,
                &[
                    "Suggested names when renaming, saving, and editing files",
                    "are based on file content and the names of other files in the",
                    "folder.",
                ],
                footnote,
            ))
            .into_any_element()
    }

    fn sidebar(&self) -> AnyElement {
        let mut pane = div().child(
            at(26., 108.)
                .w(px(300.))
                .child(label("Show these items in the sidebar:")),
        );
        let mut y = 139.;
        for (section, rows) in sidebar::shown_sections() {
            if !section.is_empty() {
                pane = pane.child(
                    at(38., y + 4.5).w(px(200.)).child(
                        label(section)
                            .font_size(tokens::small_system_font_size())
                            .weight(FontWeight::BOLD)
                            .color(SystemColor::SecondaryLabel),
                    ),
                );
                y += 27.;
            }
            for (shown, title, on) in rows {
                pane = pane
                    .child(
                        at(41., y + 1.).child(
                            native_checkbox(("ng.finder.settings.row", shown as usize), on)
                                .control_size(ControlSize::Small)
                                .on_change(move |state: &CheckState, _, cx| {
                                    sidebar::set_row_shown(shown, *state == CheckState::On);
                                    sidebar::reload_sidebars(cx);
                                }),
                        ),
                    )
                    .child(
                        at(58., y - 1.)
                            .w(px(20.))
                            .h(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(row_icon(shown)),
                    )
                    .child(
                        at(79., y - 1.)
                            .w(px(260.))
                            .child(label(&title).color(SystemColor::SecondaryLabel)),
                    );
                y += 28.;
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
            .children(
                ADVANCED
                    .iter()
                    .zip(0..)
                    .map(|((label, key, default), row)| {
                        at(28., 108. + 26. * row as f32).child(toggle(key, label, *default))
                    }),
            )
            .child(
                at(26., 238.)
                    .w(px(300.))
                    .child(label("Keep folders on top:")),
            )
            .children(FOLDERS_ON_TOP.iter().zip(0..).map(|((label, key), row)| {
                at(50., 264. + 26. * row as f32).child(toggle(key, label, false))
            }))
            .child(
                at(26., 324.)
                    .w(px(300.))
                    .child(label("When performing a search:")),
            )
            .child(
                at(50., 348.).child(
                    native_popup("ng.finder.settings.scope", scope)
                        .items(SCOPES.iter().map(|(label, _)| *label))
                        .width(px(238.))
                        .on_change(|index: &usize, _, _| {
                            prefs::set_setting_text("FXDefaultSearchScope", SCOPES[*index].1);
                        }),
                ),
            )
            .into_any_element()
    }
}

fn row_icon(shown: Shown) -> AnyElement {
    let id = ("ng.finder.settings.icon", shown as usize);
    let symbol = |name: &str| {
        native_symbol(id, name, 13., FontWeight::NORMAL)
            .tint(SystemColor::SecondaryLabel)
            .into_any_element()
    };
    // Finder's art for the rows SF Symbols has no glyph for, named for `native_image` to find.
    let art = |resource: &str| {
        let name = format!("ng.finder.settings.{resource}");
        let named = NSString::from_str(&name);
        if NSImage::imageNamed(&named).is_none() {
            if let Some(image) = system_template(resource, px(14.)) {
                image.setName(Some(&named));
            }
        }
        native_image(id, &name)
            .tint(SystemColor::SecondaryLabel)
            .into_any_element()
    };
    match shown {
        Shown::Recents => symbol("clock"),
        Shown::Shared => symbol("person.2"),
        Shown::Applications => art("SidebarApplicationsFolder"),
        Shown::Desktop => symbol("menubar.dock.rectangle"),
        Shown::Documents => symbol("doc"),
        Shown::Downloads => symbol("arrow.down.circle"),
        Shown::Movies => symbol("film"),
        Shown::Music => art("SidebarMusicFolder"),
        Shown::Pictures => symbol("photo"),
        Shown::ICloudDrive | Shown::CloudStorage => symbol("icloud"),
        Shown::Home => symbol("house"),
        Shown::Computer => symbol(&crate::locations::computer_symbol()),
        Shown::HardDisks => symbol("internaldrive"),
        Shown::ExternalDisks => symbol("externaldrive"),
        Shown::Removable => symbol("opticaldisc"),
        Shown::AirDrop => art("SidebarAirDrop"),
        Shown::Bonjour => symbol("bonjour"),
        Shown::Servers => symbol("display"),
        Shown::Trash => symbol("trash"),
        Shown::RecentTags => symbol("circlebadge.2"),
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
            1 => self.tags(cx),
            2 => self.sidebar(),
            _ => self.advanced(),
        };
        native_root(
            div()
                .size_full()
                .relative()
                .child(
                    material(
                        "ng.finder.settings.background",
                        RawMaterial::WindowBackground,
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(pane),
        )
    }
}

fn install_toolbar(pane: usize, settings: WeakEntity<Settings>, window: &mut Window, cx: &mut App) {
    let items: Vec<ToolbarItem> = PANES
        .iter()
        .enumerate()
        .map(|(index, (name, symbol))| {
            let settings = settings.clone();
            ToolbarItem::symbol(format!("ng.finder.settings.{name}"), *symbol, *name)
                .selectable(index == pane)
                .on_click(move |window, cx| {
                    settings
                        .update(cx, |this, cx| show_pane(this, index, window, cx))
                        .ok();
                })
        })
        .collect();
    set_toolbar(window, cx, Some(ToolbarStyle::Preference), &items);
}

fn show_pane(this: &mut Settings, pane: usize, window: &mut Window, cx: &mut Context<Settings>) {
    if this.pane == pane {
        return;
    }
    this.pane = pane;
    prefs::set_setting_text(PANE, PANES[pane].0);
    install_toolbar(pane, cx.entity().downgrade(), window, cx);
    fit(window, pane);
    cx.notify();
}

/// Resizes to the pane's content, keeping the top edge where it is.
fn fit(window: &Window, pane: usize) {
    let Some(ns_window) = crate::frame::ns_window(window) else {
        return;
    };
    let frame = ns_window.frame();
    let chrome = frame.size.height - ns_window.contentRectForFrameRect(frame).size.height;
    let height = chrome + f64::from(CONTENT[pane]);
    let fitted = NSRect::new(
        NSPoint::new(frame.origin.x, frame.origin.y + frame.size.height - height),
        NSSize::new(frame.size.width, height),
    );
    // The animator returns at once: a blocking resize would reach gpui while it holds the app.
    let animator: Retained<NSWindow> = unsafe { msg_send![&*ns_window, animator] };
    animator.setFrame_display(fitted, true);
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
                titlebar: Some(TitlebarOptions {
                    title: Some("Found’er Settings".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(969.), px(350.)),
                    size: size(px(WIDTH), px(CONTENT[pane])),
                })),
                is_resizable: false,
                is_minimizable: false,
                ..window_options()
            },
            |window, cx| {
                let entity = cx.new(|_| Settings {
                    pane,
                    tag_picked: None,
                    tag_draft: None,
                    tag_fresh: false,
                    tag_scroll: UniformListScrollHandle::new(),
                });
                install_toolbar(pane, entity.downgrade(), window, cx);
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
