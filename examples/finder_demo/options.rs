use std::cell::Cell;
use std::path::Path;

use finder_fs::ds_store::Background;
use gpui::{
    AnyElement, App, Bounds, Context, ElementId, Entity, ExternalPaths, FontWeight, Hsla, Render,
    Rgba, SharedString, WeakEntity, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions,
    div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, FileType, SystemColor, system_color};
use native_gpui::{
    CheckState, ControlSize, MenuEntry, NativeCheckbox, NativePopup, NativeSlider, native_button,
    native_checkbox, native_color_well, native_image, native_label, native_popup, native_radio,
    native_raster, native_root, native_slider, native_symbol, window_options,
};
use objc2::msg_send;
use objc2_app_kit::{
    NSBundleImageExtension, NSModalResponseOK, NSOpenPanel, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::{MainThreadMarker, NSBundle, NSString};

use crate::app::FinderDemo;
use crate::data::{Column, GROUP_KEYS, SORT_KEYS, SORT_TITLES};
use crate::frame::ns_window;
use crate::prefs;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};
use crate::views::header::column_title;
use crate::views::icon::{Arrangement, SIZE_STEPS, size_slider, slider_size};

const TEXT_SIZES: [f32; 7] = [10., 11., 12., 13., 14., 15., 16.];

const ARRANGEMENTS: [Option<Arrangement>; 12] = [
    Some(Arrangement::Free),
    None,
    Some(Arrangement::Grid),
    None,
    Some(Arrangement::By(Column::Name)),
    Some(Arrangement::By(Column::Kind)),
    Some(Arrangement::By(Column::Opened)),
    Some(Arrangement::By(Column::Added)),
    Some(Arrangement::By(Column::Modified)),
    Some(Arrangement::By(Column::Created)),
    Some(Arrangement::By(Column::Size)),
    Some(Arrangement::By(Column::Tags)),
];

/// `None` rows are iCloud Drive's alone.
const SHOW_COLUMNS: [(&str, Option<Column>); 12] = [
    ("iCloud Status", None),
    ("Last Modified By", None),
    ("Shared By", Some(Column::SharedBy)),
    ("Date Modified", Some(Column::Modified)),
    ("Date Created", Some(Column::Created)),
    ("Date Last Opened", Some(Column::Opened)),
    ("Date Added", Some(Column::Added)),
    ("Size", Some(Column::Size)),
    ("Kind", Some(Column::Kind)),
    ("Version", Some(Column::Version)),
    ("Comments", Some(Column::Comments)),
    ("Tags", Some(Column::Tags)),
];

const THUMBS: [(f32, &str, &str, (f32, f32), f32); 3] = [
    (
        32.,
        "viewOptionsThumbnail32",
        "small thumbnail size",
        (20., 174.),
        29.,
    ),
    (
        48.,
        "viewOptionsThumbnail48",
        "medium thumbnail size",
        (78., 161.),
        95.,
    ),
    (
        64.,
        "viewOptionsThumbnail64",
        "large thumbnail size",
        (152., 148.),
        177.,
    ),
];

const SMALL: ControlSize = ControlSize::Small;

pub(crate) struct ViewOptions {
    demo: WeakEntity<FinderDemo>,
}

type Change<T> = fn(&mut FinderDemo, T, &mut Context<FinderDemo>);

fn view_name(view: usize) -> &'static str {
    match view {
        ICON_VIEW => "icon",
        LIST_VIEW => "list",
        COLUMN_VIEW => "column",
        _ => "gallery",
    }
}

fn at(x: f32, y: f32, child: impl IntoElement) -> AnyElement {
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .child(child)
        .into_any_element()
}

fn small_popup(popup: NativePopup, width: f32) -> NativePopup {
    popup
        .control_size(SMALL)
        .font_size(tokens::system_font_size_for(SMALL))
        .width(px(width))
}

fn document(id: &'static str, side: f32) -> Option<AnyElement> {
    let icon = crate::icons::icon(FileType::Uti("public.data"), side)?;
    Some(native_raster(id, icon).into_any_element())
}

fn name_thumbnails() {
    thread_local! {
        static NAMED: Cell<bool> = const { Cell::new(false) };
    }
    if NAMED.replace(true) {
        return;
    }
    let finder = NSString::from_str("/System/Library/CoreServices/Finder.app");
    let Some(finder) = NSBundle::bundleWithPath(&finder) else {
        return;
    };
    for (_, name, description, ..) in THUMBS {
        let name = NSString::from_str(name);
        if let Some(image) = finder.imageForResource(&name) {
            image.setName(Some(&name));
            image.setAccessibilityDescription(Some(&NSString::from_str(description)));
        }
    }
}

fn shows(demo: &FinderDemo, column: Column) -> bool {
    demo.list_columns.iter().any(|spec| spec.column == column)
}

fn set_background(demo: &mut FinderDemo, background: Background, cx: &mut Context<FinderDemo>) {
    demo.change_icon_options(|options| options.background = Some(background), cx);
}

struct Build<'a> {
    demo: &'a Entity<FinderDemo>,
    state: &'a FinderDemo,
    out: Vec<AnyElement>,
}

impl Build<'_> {
    fn run<T: 'static>(&self, change: Change<T>) -> impl Fn(T, &mut App) + 'static {
        let demo = self.demo.clone();
        move |value, cx| {
            demo.update(cx, |this, cx| {
                change(this, value, cx);
                cx.notify();
            })
        }
    }

    fn put(&mut self, element: impl Into<Option<AnyElement>>) {
        self.out.extend(element.into());
    }

    fn label(&mut self, id: &'static str, (y, width): (f32, f32), text: &str) {
        let text = native_label(id, text).font_size(tokens::system_font_size_for(SMALL));
        self.put(at(18., y, div().w(px(width)).child(text)));
    }

    fn checkbox(
        &self,
        id: &'static str,
        title: impl Into<String>,
        on: bool,
        change: Change<bool>,
    ) -> NativeCheckbox {
        let run = self.run(change);
        native_checkbox(id, on)
            .title(title)
            .control_size(SMALL)
            .on_change(move |state: &CheckState, _, cx| run(*state == CheckState::On, cx))
    }

    fn check(
        &mut self,
        id: &'static str,
        y: f32,
        title: impl Into<String>,
        on: bool,
        change: Change<bool>,
    ) {
        let checkbox = self.checkbox(id, title, on, change);
        self.put(at(20., y, checkbox));
    }

    fn radio<T: Copy + 'static>(
        &mut self,
        id: impl Into<ElementId>,
        (x, y): (f32, f32),
        title: &str,
        (value, chosen): (T, bool),
        change: Change<T>,
    ) {
        let run = self.run(change);
        self.put(at(
            x,
            y,
            native_radio(id, chosen)
                .title(title)
                .control_size(SMALL)
                .on_change(move |_: &bool, _, cx| run(value, cx)),
        ));
    }

    fn pick<T: Copy + 'static>(
        &mut self,
        id: &'static str,
        (x, y): (f32, f32),
        image: Option<AnyElement>,
        (value, change): (T, Change<T>),
    ) {
        let Some(image) = image else {
            return;
        };
        let run = self.run(change);
        self.put(
            div()
                .id(id)
                .absolute()
                .left(px(x))
                .top(px(y))
                .child(image)
                .on_click(move |_, _, cx| run(value, cx))
                .into_any_element(),
        );
    }

    fn slider(&mut self, slider: NativeSlider, y: f32, change: Change<f64>) {
        let run = self.run(change);
        let slider = slider
            .control_size(SMALL)
            .on_change(move |value: &f64, _, cx| run(*value, cx));
        self.put(at(44., y, div().w(px(132.)).child(slider)));
    }

    fn text_size(&mut self, y: f32, size: f32, change: Change<f32>) {
        self.label("ng.finder.options.text.label", (y + 3., 54.), "Text size:");
        let chosen = TEXT_SIZES.iter().position(|at| *at == size.round());
        let run = self.run(change);
        let popup = native_popup("ng.finder.options.text", chosen.unwrap_or(2))
            .entries(
                TEXT_SIZES
                    .iter()
                    .map(|size| MenuEntry::item(size.to_string())),
            )
            .on_change(move |index: &usize, _, cx| {
                if let Some(size) = TEXT_SIZES.get(*index).copied() {
                    run(size, cx);
                }
            });
        self.put(at(77.5, y, small_popup(popup, 64.)));
    }

    fn group(&mut self) {
        self.label("ng.finder.options.group.label", (92., 57.), "Group By:");
        let here = self.state.here();
        let keys: Vec<Column> = (GROUP_KEYS.into_iter())
            .filter(|key| key.offered_at(here))
            .collect();
        let chosen = self
            .state
            .group()
            .and_then(|group| keys.iter().position(|key| *key == group))
            .map_or(0, |at| at + 2);
        let entries = [MenuEntry::item("None"), MenuEntry::separator()]
            .into_iter()
            .chain(keys.iter().map(|key| MenuEntry::item(column_title(*key))));
        let run = self.run(|this, group, cx| this.set_group(group, cx));
        let popup = native_popup("ng.finder.options.group", chosen)
            .entries(entries)
            .on_change(move |index: &usize, _, cx| match index.checked_sub(2) {
                None => run(None, cx),
                Some(at) => {
                    if let Some(key) = keys.get(at) {
                        run(Some(*key), cx);
                    }
                }
            });
        self.put(at(82., 89., small_popup(popup, 134.)));
    }

    fn sort(&mut self, y: f32) {
        self.label("ng.finder.options.sort.label", (y + 3., 46.), "Sort By:");
        let popup = match self
            .state
            .arrangement()
            .filter(|_| self.state.view == ICON_VIEW)
        {
            Some(arranged) => self.arrange_popup(arranged),
            None => {
                let chosen = SORT_KEYS.iter().position(|key| *key == self.state.sort);
                let run = self.run(|this, column: Column, cx| {
                    if this.sort != column {
                        this.resort((column, column.first_ascending()), cx);
                    }
                });
                native_popup("ng.finder.options.sort", chosen.unwrap_or(0))
                    .entries(SORT_TITLES.iter().map(|title| MenuEntry::item(*title)))
                    .on_change(move |index: &usize, _, cx| {
                        if let Some(column) = SORT_KEYS.get(*index).copied() {
                            run(column, cx);
                        }
                    })
            }
        };
        self.put(at(82., y, small_popup(popup, 134.)));
    }

    fn arrange_popup(&self, arranged: Arrangement) -> NativePopup {
        let entries = ARRANGEMENTS.iter().map(|arrangement| match arrangement {
            None => MenuEntry::separator(),
            Some(Arrangement::Free) => MenuEntry::item("None"),
            Some(Arrangement::Grid) => MenuEntry::item("Snap to Grid"),
            Some(Arrangement::By(column)) => MenuEntry::item(column_title(*column)),
        });
        let chosen = ARRANGEMENTS
            .iter()
            .position(|arrangement| *arrangement == Some(arranged));
        let run = self.run(|this, arrangement, cx| this.arrange(arrangement, cx));
        native_popup("ng.finder.options.arrange", chosen.unwrap_or(0))
            .entries(entries)
            .on_change(move |index: &usize, _, cx| {
                if let Some(arrangement) = ARRANGEMENTS.get(*index).copied().flatten() {
                    run(arrangement, cx);
                }
            })
    }

    fn previews(&mut self, y: f32) {
        self.check(
            "ng.finder.options.previews",
            y,
            "Show icon preview",
            self.state.icon_previews(),
            |this, on, cx| this.set_icon_previews(on, cx),
        );
    }

    fn preview_column(&mut self, y: f32) {
        self.check(
            "ng.finder.options.preview",
            y,
            "Show preview column",
            self.state.preview,
            |this, on, _| this.show_preview(on),
        );
    }

    fn list(&mut self) {
        let look = self.state.list_look;
        self.label("ng.finder.options.icon.label", (172., 54.), "Icon size:");
        let change: Change<f32> =
            |this, icon, cx| this.change_list_look(|look| look.icon = icon, cx);
        for (side, id, (x, y), radio) in [
            (16., "ng.finder.options.icon.small", (94., 170.), 95.),
            (32., "ng.finder.options.icon.large", (131., 154.), 140.),
        ] {
            let small = side < 24.;
            self.pick(id, (x, y), document(id, side), (side, change));
            self.radio(
                ("ng.finder.options.icon.size", usize::from(small)),
                (radio, 194.),
                "",
                (side, (look.icon < 24.) == small),
                change,
            );
        }
        self.text_size(218., look.text, |this, text, cx| {
            this.change_list_look(|look| look.text = text, cx)
        });
        self.label("ng.finder.options.columns", (246., 85.), "Show Columns:");
        let here = self.state.here();
        for (row, &(title, column)) in SHOW_COLUMNS.iter().enumerate() {
            let column = column.filter(|column| column.offered_at(here));
            let demo = self.demo.clone();
            self.put(at(
                20.,
                264. + 20. * row as f32,
                native_checkbox(
                    ("ng.finder.options.column", row),
                    column.is_some_and(|column| shows(self.state, column)),
                )
                .title(title)
                .control_size(SMALL)
                .enabled(column.is_some())
                .on_change(move |state: &CheckState, _, cx| {
                    let Some(column) = column else {
                        return;
                    };
                    demo.update(cx, |this, cx| {
                        if shows(this, column) != (*state == CheckState::On) {
                            this.toggle_column(column, cx);
                            cx.notify();
                        }
                    });
                }),
            ));
        }
        self.check(
            "ng.finder.options.relative",
            515.,
            "Use relative dates",
            look.relative,
            |this, on, cx| this.change_list_look(|look| look.relative = on, cx),
        );
        self.check(
            "ng.finder.options.sizes",
            535.,
            "Calculate all sizes",
            look.sizes,
            |this, on, cx| this.change_list_look(|look| look.sizes = on, cx),
        );
        self.previews(555.);
    }

    fn column(&mut self) {
        let look = self.state.column_look;
        self.text_size(154., look.text, |this, text, cx| {
            this.change_column_look(|look| look.text = text, cx)
        });
        self.check(
            "ng.finder.options.icons",
            182.,
            "Show icons",
            look.icons,
            |this, on, cx| this.change_column_look(|look| look.icons = on, cx),
        );
        self.previews(202.);
        self.preview_column(222.);
        self.check(
            "ng.finder.options.fit",
            242.,
            "Resize columns to fit filenames",
            look.fit,
            |this, on, cx| this.change_column_look(|look| look.fit = on, cx),
        );
    }

    fn gallery(&mut self) {
        name_thumbnails();
        let look = self.state.gallery_look;
        self.label(
            "ng.finder.options.thumb.label",
            (126., 87.),
            "Thumbnail size:",
        );
        let change: Change<f32> =
            |this, thumb, cx| this.change_gallery_look(|look| look.thumb = thumb, cx);
        for (index, (side, name, _, (x, y), radio)) in THUMBS.into_iter().enumerate() {
            let image = native_image(("ng.finder.options.thumb", index), name);
            self.pick(name, (x, y), Some(image.into_any_element()), (side, change));
            self.radio(
                ("ng.finder.options.thumb.size", index),
                (radio, 208.),
                "",
                (side, look.thumb == side),
                change,
            );
        }
        self.preview_column(239.);
        self.previews(259.);
        self.check(
            "ng.finder.options.filename",
            279.,
            "Show filename",
            look.filename,
            |this, on, cx| this.change_gallery_look(|look| look.filename = on, cx),
        );
    }

    fn icon(&mut self) {
        let options = self.state.icon_options();
        self.label("ng.finder.options.icon.label", (154., 54.), "Icon size:");
        let points = native_label(
            "ng.finder.options.icon.points",
            &format!("{0} \u{d7} {0}", options.size),
        )
        .font_size(tokens::system_font_size_for(SMALL))
        .color(SystemColor::SecondaryLabel);
        self.put(at(76., 154., div().w(px(42.)).child(points)));
        self.put(document("ng.finder.options.icon.small", 16.).map(|doc| at(20., 172., doc)));
        let size = native_slider("ng.finder.options.size", size_slider(options.size))
            .range(0., SIZE_STEPS)
            .ticks(9);
        self.slider(size, 171., |this, step, cx| {
            this.change_icon_options(|options| options.icon_size = Some(slider_size(step)), cx)
        });
        self.put(document("ng.finder.options.icon.large", 32.).map(|doc| at(184., 164., doc)));
        self.label(
            "ng.finder.options.spacing.label",
            (199., 74.),
            "Grid spacing:",
        );
        self.put(at(
            20.,
            217.,
            native_symbol(
                "ng.finder.options.spacing.small",
                "square.grid.3x3",
                13.,
                FontWeight::NORMAL,
            ),
        ));
        let spacing = native_slider("ng.finder.options.spacing", f64::from(options.spacing))
            .range(1., 100.)
            .ticks(8);
        self.slider(spacing, 216., |this, spacing, cx| {
            this.change_icon_options(
                |options| options.grid_spacing = Some(spacing.round() as f32),
                cx,
            )
        });
        self.put(at(
            185.,
            210.,
            native_symbol(
                "ng.finder.options.spacing.large",
                "square.grid.2x2",
                26.,
                FontWeight::NORMAL,
            ),
        ));
        self.text_size(258., options.text, |this, text, cx| {
            this.change_icon_options(|options| options.text_size = Some(text), cx)
        });
        self.label(
            "ng.finder.options.label.label",
            (286., 81.),
            "Label position:",
        );
        let placed: Change<bool> = |this, bottom, cx| {
            this.change_icon_options(|options| options.label_on_bottom = Some(bottom), cx)
        };
        for (bottom, x, title) in [(true, 20., "Bottom"), (false, 84.5, "Right")] {
            self.radio(
                ("ng.finder.options.label", usize::from(bottom)),
                (x, 307.),
                title,
                (bottom, options.bottom == bottom),
                placed,
            );
        }
        self.check(
            "ng.finder.options.info",
            338.,
            "Show item info",
            options.info,
            |this, on, cx| {
                this.change_icon_options(|options| options.show_item_info = Some(on), cx)
            },
        );
        self.previews(358.);
        self.background();
    }

    fn background(&mut self) {
        self.label(
            "ng.finder.options.background.label",
            (409., 70.),
            "Background:",
        );
        let chosen: Change<usize> = |this, kind, cx| {
            let background = match (kind, this.icon_paint().background) {
                (0, _) => Background::Default,
                (1, kept @ Background::Color(_)) => kept,
                (1, _) => Background::Color([1., 1., 1.]),
                (_, kept @ Background::Picture(_)) => kept,
                _ => Background::Picture(String::new()),
            };
            set_background(this, background, cx)
        };
        let background = self.state.icon_paint().background;
        let kind = match background {
            Background::Default => 0,
            Background::Color(_) => 1,
            Background::Picture(_) => 2,
        };
        for (index, title) in ["Default", "Color", "Picture"].into_iter().enumerate() {
            self.radio(
                ("ng.finder.options.background", index),
                (20., 426. + 18. * index as f32),
                title,
                (index, kind == index),
                chosen,
            );
        }
        match background {
            Background::Color([r, g, b]) => {
                let run = self.run(set_background);
                let color = Rgba { r, g, b, a: 1. };
                let well = native_color_well("ng.finder.options.color", color.into())
                    .supports_alpha(false)
                    .on_change(move |color: &Hsla, _, cx| {
                        let rgb = color.to_rgb();
                        run(Background::Color([rgb.r, rgb.g, rgb.b]), cx)
                    });
                self.put(at(93., 440., well));
            }
            Background::Picture(path) => self.picture_well(&path),
            Background::Default => {}
        }
    }

    fn picture_well(&mut self, path: &str) {
        let demo = self.demo.clone();
        let dropped = self.run(set_background);
        let named = Path::new(path)
            .file_name()
            .map(|name| SharedString::from(name.to_string_lossy().into_owned()));
        self.put(
            div()
                .id("ng.finder.options.picture")
                .absolute()
                .left(px(93.))
                .top(px(461.))
                .size(px(64.))
                .rounded(px(4.))
                .border_1()
                .border_color(system_color(SystemColor::Separator))
                .flex()
                .items_center()
                .justify_center()
                .p(px(4.))
                .text_size(tokens::small_system_font_size())
                .text_color(system_color(SystemColor::SecondaryLabel))
                .text_center()
                .child(named.unwrap_or_else(|| "Drag image here".into()))
                .on_click(move |_, _, cx| demo.update(cx, |_, cx| choose_picture(cx)))
                .on_drop(move |paths: &ExternalPaths, _, cx| {
                    if let Some(path) = paths.paths().first() {
                        let path = path.to_string_lossy().into_owned();
                        dropped(Background::Picture(path), cx);
                    }
                })
                .into_any_element(),
        );
    }
}

fn choose_picture(cx: &mut Context<FinderDemo>) {
    // From a task, outside GPUI's borrow of the app, which the modal loop would re-enter.
    cx.spawn(async move |this, cx| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(mtm);
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(path) = panel.URL().and_then(|url| url.path()) else {
            return;
        };
        let picture = Background::Picture(path.to_string());
        this.update(cx, |this, cx| set_background(this, picture, cx))
            .ok();
    })
    .detach();
}

impl Render for ViewOptions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(demo) = self.demo.upgrade() else {
            return div().into_any_element();
        };
        let state = demo.read(cx);
        let title = state
            .folder()
            .map(|folder| folder.title)
            .unwrap_or("Finder");
        window.set_window_title(title);
        let (view, here) = (state.view, state.here());
        let name = view_name(view);
        let opens = prefs::opens_in(here).filter(|(opens, _)| *opens == view);
        let mut build = Build {
            demo: &demo,
            state,
            out: Vec::new(),
        };
        build.check(
            "ng.finder.options.always",
            36.,
            format!("Always open in {name} view"),
            opens.is_some(),
            |this, on, _| this.always_open(on),
        );
        let browse = build.checkbox(
            "ng.finder.options.browse",
            format!("Browse in {name} view"),
            opens.is_some_and(|(_, browse)| browse),
            |this, on, _| this.always_browse(on),
        );
        build.put(at(30., 58., browse.enabled(opens.is_some())));
        match view {
            GALLERY_VIEW => build.sort(89.),
            _ => {
                build.group();
                build.sort(117.);
            }
        }
        match view {
            LIST_VIEW => build.list(),
            COLUMN_VIEW => build.column(),
            GALLERY_VIEW => build.gallery(),
            _ => build.icon(),
        }
        if view != COLUMN_VIEW {
            let run = build.run(|this, (), _| prefs::use_as_defaults(this.view, this.here()));
            build.put(at(
                57.5,
                606.,
                native_button("ng.finder.options.defaults", "Use as Defaults")
                    .control_size(SMALL)
                    .width(px(121.))
                    .enabled(prefs::has_own_look(view, here))
                    .on_click(move |_, _, cx| run((), cx)),
            ));
        }

        native_root(
            div()
                .size_full()
                .relative()
                .bg(system_color(SystemColor::WindowBackground))
                .children(build.out),
        )
        .into_any_element()
    }
}

thread_local! {
    static OPEN: Cell<Option<WindowHandle<ViewOptions>>> = const { Cell::new(None) };
}

pub(crate) fn shown(cx: &App) -> bool {
    OPEN.get().is_some_and(|open| {
        cx.windows()
            .iter()
            .any(|window| window.window_id() == open.window_id())
    })
}

pub(crate) fn open(demo: &Entity<FinderDemo>, cx: &mut App) {
    if shown(cx) {
        if let Some(open) = OPEN.take() {
            let _ = open.update(cx, |_, window, _| window.remove_window());
        }
        return;
    }
    let watched = demo.clone();
    let demo = demo.downgrade();
    let opened = cx.open_window(
        WindowOptions {
            kind: WindowKind::Floating,
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(320.), px(220.)),
                size: size(px(236.), px(646.)),
            })),
            is_resizable: false,
            is_minimizable: false,
            focus: false,
            ..window_options()
        },
        |window, cx| {
            if let Some(panel) = ns_window(window) {
                unsafe {
                    let _: () = msg_send![&*panel, setFloatingPanel: true];
                    let _: () = msg_send![&*panel, setBecomesKeyOnlyIfNeeded: true];
                }
                panel.setStyleMask(
                    panel.styleMask()
                        | NSWindowStyleMask::UtilityWindow
                        | NSWindowStyleMask::NonactivatingPanel,
                );
                panel.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
            cx.new(|cx| {
                cx.observe_in(&watched, window, |_, _, _, cx| cx.notify())
                    .detach();
                ViewOptions { demo }
            })
        },
    );
    OPEN.set(opened.ok());
}
