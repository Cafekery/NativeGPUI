use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, FontWeight, IntoElement, ParentElement,
    Render, Styled, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, div, point,
    px, rgb, size,
};
use native_gpui::tokens::{self, Appearance, FileType, SystemColor};
use native_gpui::{
    CheckState, ControlSize, MenuEntry, NativeLabel, RawMaterial, ToolbarItem, ToolbarStyle,
    material, native_checkbox, native_image, native_label, native_popup, native_root, set_toolbar,
    system_symbol, window_options,
};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSImage, NSImageSymbolConfiguration, NSWindow};
use objc2_foundation::{
    NSPoint, NSRect, NSSize, NSString, NSURL, NSURLIsUbiquitousItemKey, NSUserDefaults,
};

use crate::app::FinderDemo;
use crate::chrome::sidebar::{self, Shown};
use crate::prefs::{self, WindowTarget};

mod tags_pane;

const WIDTH: f32 = 377.;
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

const FOLDERS_ON_TOP: [(&str, &str, bool); 2] = [
    (
        "In windows when sorting by name",
        "_FXSortFoldersFirst",
        false,
    ),
    ("On Desktop", "_FXSortFoldersFirstOnDesktop", false),
];

const SCOPES: [(&str, &str); 3] = [
    ("Search This Mac", "SCev"),
    ("Search the Current Folder", "SCcf"),
    ("Use the Previous Search Scope", "SCsp"),
];

pub(crate) struct Settings {
    pane: usize,
    tag_list: tags_pane::TagList,
}

enum Icon {
    Path(PathBuf),
    Art(FileType<'static>),
}

#[derive(Default)]
struct Place {
    title: String,
    icon: Option<Icon>,
    code: Option<&'static str>,
    path: Option<PathBuf>,
}

impl Place {
    fn entry(&self) -> MenuEntry {
        if self.code.is_none() {
            return MenuEntry::separator();
        }
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
    let separator = Place::default;
    let name = finder_fs::name::of_path;
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

fn placed(x: f32, y: f32) -> Div {
    div().absolute().left(px(x)).top(px(y))
}

fn at(x: f32, y: f32) -> Div {
    placed(x, y - CHROME)
}

fn label(text: &str) -> NativeLabel {
    native_label(format!("ng.finder.settings.{text}"), text)
}

fn small(text: &str) -> NativeLabel {
    label(text).font_size(tokens::small_system_font_size())
}

fn footnote(text: &str) -> NativeLabel {
    label(text)
        .font_size(px(10.))
        .color(SystemColor::SecondaryLabel)
}

fn heading(x: f32, y: f32, text: &str) -> Div {
    at(x, y).w(px(300.)).child(label(text))
}

fn lines(
    x: f32,
    y: f32,
    pitch: f32,
    texts: &'static [&'static str],
    make: fn(&str) -> NativeLabel,
) -> impl Iterator<Item = Div> {
    texts.iter().zip(0..).map(move |(text, line)| {
        at(x, y + pitch * line as f32)
            .w(px(WIDTH - x))
            .child(make(text))
    })
}

fn toggles(
    x: f32,
    y: f32,
    pitch: f32,
    rows: &'static [(&'static str, &'static str, bool)],
) -> impl Iterator<Item = Div> {
    rows.iter()
        .zip(0..)
        .map(move |((label, key, default), row)| {
            at(x, y + pitch * row as f32).child(toggle(key, label, *default))
        })
}

fn global_on(key: &str) -> bool {
    NSUserDefaults::standardUserDefaults().boolForKey(&NSString::from_str(key))
}

fn syncs_desktop() -> bool {
    finder_fs::volumes::standard(finder_fs::volumes::Standard::Desktop)
        .is_some_and(|desktop| crate::info::resource(&desktop, unsafe { NSURLIsUbiquitousItemKey }))
}

fn toggle(key: &'static str, label: &str, default: bool) -> impl IntoElement {
    native_checkbox(
        format!("ng.finder.settings.{key}"),
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
        let entries: Vec<MenuEntry> = rows.iter().map(Place::entry).collect();
        let picked = cx.listener(move |_, index: &usize, window, cx| {
            let Some(place) = rows.get(*index) else {
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
            .child(heading(28., 104., "Show these items on the desktop:"))
            .children(toggles(48., 128., 22., &DESKTOP))
            .child(heading(28., 234., "New Finder windows show:"))
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
            .children(lines(
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
            .children(lines(
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

    fn sidebar(&self, key: bool) -> AnyElement {
        let mut pane = div().child(heading(26., 108., "Show these items in the sidebar:"));
        let mut y = 139.;
        for (section, rows) in sidebar::shown_sections() {
            if !section.is_empty() {
                pane = pane.child(
                    at(38., y + 4.5).w(px(200.)).child(
                        small(section)
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
                    .child(row_icon(shown, key, y))
                    .child(
                        at(79., y - 1.)
                            .w(px(260.))
                            .child(label(&title).disabled(!key)),
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
            .children(toggles(28., 108., 26., &ADVANCED))
            .child(heading(26., 238., "Keep folders on top:"))
            .children(toggles(50., 264., 26., &FOLDERS_ON_TOP))
            .child(heading(26., 324., "When performing a search:"))
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

fn row_icon(shown: Shown, key: bool, y: f32) -> Div {
    let computer;
    let symbol = match shown {
        Shown::Recents => "clock",
        Shown::Shared => "person.2",
        Shown::Applications => "appstore",
        Shown::Desktop => "menubar.dock.rectangle",
        Shown::Documents => "doc",
        Shown::Downloads => "arrow.down.circle",
        Shown::Movies => "film",
        Shown::Music => "music",
        Shown::Pictures => "photo",
        Shown::ICloudDrive | Shown::CloudStorage => "icloud",
        Shown::Home => "house",
        Shown::Computer => {
            computer = crate::locations::computer_symbol();
            &computer
        }
        Shown::HardDisks => "internaldrive",
        Shown::ExternalDisks => "externaldrive",
        Shown::Removable => "opticaldisc",
        Shown::AirDrop => "airdrop",
        Shown::Bonjour => "bonjour",
        Shown::Servers => "pc",
        Shown::Trash => "trash",
        Shown::RecentTags => "circlebadge.2",
    };
    let name = format!("ng.finder.settings.{symbol}");
    let named = NSString::from_str(&name);
    let Some(image) = NSImage::imageNamed(&named).or_else(|| {
        let look = NSImageSymbolConfiguration::configurationWithPointSize_weight(13., 0.);
        let image = system_symbol(symbol)?.imageWithSymbolConfiguration(&look)?;
        image.setName(Some(&named));
        Some(image)
    }) else {
        return div();
    };
    let size = image.size();
    let baseline = image.alignmentRect().origin.y;
    // Finder sits each symbol on its label's baseline, centred to the whole point.
    let x = 68. - (size.width / 2.).floor();
    let top = f64::from(y) + 12. - (size.height - baseline);
    let icon = native_image(("ng.finder.settings.icon", shown as usize), &name);
    let icon = match (key, tokens::current_appearance()) {
        (false, _) => icon.untinted(),
        (true, Appearance::Dark) => icon.tint(SystemColor::Label),
        // Finder inks light rows calibrated white 0.01, which draws as sRGB 3, not black.
        (true, _) => icon.ink(rgb(0x030303)),
    };
    at(x as f32, top as f32).child(icon)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = match self.pane {
            0 => self.general(cx),
            1 => self.tags(cx),
            2 => self.sidebar(window.is_window_active()),
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
    let window = open.or_else(|| {
        let remembered = prefs::setting_text(PANE);
        let pane = PANES
            .iter()
            .position(|(name, _)| Some(*name) == remembered.as_deref())
            .unwrap_or(0);
        cx.open_window(
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
                let entity = cx.new(|cx| {
                    cx.observe_window_activation(window, |_, _, cx| cx.notify())
                        .detach();
                    Settings {
                        pane,
                        tag_list: Default::default(),
                    }
                });
                crate::chrome::toolbar::system_corners(window);
                install_toolbar(pane, entity.downgrade(), window, cx);
                entity
            },
        )
        .ok()
    });
    if let Some(window) = window {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
    }
}
