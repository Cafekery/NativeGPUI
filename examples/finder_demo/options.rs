use std::cell::Cell;
use std::path::PathBuf;

use finder_fs::ds_store::Background;
use gpui::{
    AnyElement, App, AsyncApp, Bounds, Context, Entity, ExternalPaths, Hsla, Render, Rgba,
    SharedString, WeakEntity, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div,
    point, prelude::*, px, size,
};
use native_gpui::tokens::{self, FileType, SystemColor, system_color};
use native_gpui::{
    CheckState, MenuEntry, NativePopup, native_button, native_checkbox, native_color_well,
    native_image, native_label, native_popup, native_radio, native_raster, native_root,
    native_slider, native_symbol, window_options,
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

/// Finder's own images for the three thumbnail sizes, where Finder draws them.
const THUMBS: [(f32, &str, &str, (f32, f32), f32); 3] = [
    (
        32.,
        "viewOptionsThumbnail32",
        "small thumbnail size",
        (20., 174.),
        28.,
    ),
    (
        48.,
        "viewOptionsThumbnail48",
        "medium thumbnail size",
        (78., 161.),
        94.,
    ),
    (
        64.,
        "viewOptionsThumbnail64",
        "large thumbnail size",
        (152., 148.),
        176.,
    ),
];

const PANEL: (f32, f32) = (236., 646.);

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

fn label(id: &'static str, (x, y, width): (f32, f32, f32), text: &str) -> AnyElement {
    at(x, y, div().w(px(width)).child(native_label(id, text)))
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

fn color_of([red, green, blue]: [f32; 3]) -> Hsla {
    Rgba {
        r: red,
        g: green,
        b: blue,
        a: 1.,
    }
    .into()
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

    fn check(
        &mut self,
        id: &'static str,
        (x, y): (f32, f32),
        title: &str,
        on: bool,
        change: Change<bool>,
    ) {
        let run = self.run(change);
        self.put(at(
            x,
            y,
            native_checkbox(id, on)
                .title(title)
                .on_change(move |state: &CheckState, _, cx| run(*state == CheckState::On, cx)),
        ));
    }

    fn radio<T: Copy + 'static>(
        &mut self,
        id: impl Into<gpui::ElementId>,
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

    fn text_size(&mut self, (x, y): (f32, f32), size: f32, change: Change<f32>) {
        self.put(label(
            "ng.finder.options.text.label",
            (19., y + 3., 52.),
            "Text size:",
        ));
        let chosen = TEXT_SIZES.iter().position(|at| *at == size.round());
        let run = self.run(change);
        self.put(at(
            x,
            y,
            native_popup("ng.finder.options.text", chosen.unwrap_or(2))
                .width(px(66.))
                .entries(
                    TEXT_SIZES
                        .iter()
                        .map(|size| MenuEntry::item(size.to_string())),
                )
                .on_change(move |index: &usize, _, cx| {
                    if let Some(size) = TEXT_SIZES.get(*index).copied() {
                        run(size, cx);
                    }
                }),
        ));
    }

    fn group(&mut self) {
        self.put(label(
            "ng.finder.options.group.label",
            (19., 91., 55.),
            "Group By:",
        ));
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
        self.put(at(
            81.,
            88.,
            native_popup("ng.finder.options.group", chosen)
                .width(px(136.))
                .entries(entries)
                .on_change(move |index: &usize, _, cx| match index.checked_sub(2) {
                    None => run(None, cx),
                    Some(at) => {
                        if let Some(key) = keys.get(at) {
                            run(Some(*key), cx);
                        }
                    }
                }),
        ));
    }

    fn sort(&mut self, y: f32) {
        self.put(label(
            "ng.finder.options.sort.label",
            (19., y + 3., 44.),
            "Sort By:",
        ));
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
        self.put(at(81., y, popup.width(px(136.))));
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
        let on = self.state.icon_previews();
        self.check(
            "ng.finder.options.previews",
            (19., y),
            "Show icon preview",
            on,
            |this, on, cx| this.set_icon_previews(on, cx),
        );
    }

    fn preview_column(&mut self, y: f32) {
        let on = self.state.preview;
        self.check(
            "ng.finder.options.preview",
            (19., y),
            "Show preview column",
            on,
            |this, on, _| this.show_preview(on),
        );
    }

    fn list(&mut self) {
        let look = self.state.list_look();
        self.put(label(
            "ng.finder.options.icon.label",
            (19., 171., 52.),
            "Icon size:",
        ));
        for (side, (x, y), radio) in [(16., (94., 170.), 94.), (32., (131., 154.), 139.)] {
            let small = side < 24.;
            let id = if small {
                "ng.finder.options.icon.small"
            } else {
                "ng.finder.options.icon.large"
            };
            let change: Change<f32> =
                |this, icon, cx| this.change_list_look(|look| look.icon = icon, cx);
            self.pick(id, (x, y), document(id, side), (side, change));
            self.radio(
                ("ng.finder.options.icon.size", usize::from(small)),
                (radio, 193.),
                "",
                (side, (look.icon < 24.) == small),
                change,
            );
        }
        self.text_size((76.5, 217.), look.text, |this, text, cx| {
            this.change_list_look(|look| look.text = text, cx)
        });
        self.put(label(
            "ng.finder.options.columns",
            (19., 245., 83.),
            "Show Columns:",
        ));
        let shown: Vec<Column> = self
            .state
            .list_columns
            .iter()
            .map(|spec| spec.column)
            .collect();
        let here = self.state.here();
        for (at_row, &(title, column)) in SHOW_COLUMNS.iter().enumerate() {
            let column = column.filter(|column| column.offered_at(here));
            let demo = self.demo.clone();
            self.put(at(
                19.,
                263. + 20. * at_row as f32,
                native_checkbox(
                    ("ng.finder.options.column", at_row),
                    column.is_some_and(|column| shown.contains(&column)),
                )
                .title(title)
                .enabled(column.is_some())
                .on_change(move |state: &CheckState, _, cx| {
                    let Some(column) = column else {
                        return;
                    };
                    demo.update(cx, |this, cx| {
                        let shown = this.list_columns.iter().any(|spec| spec.column == column);
                        if shown != (*state == CheckState::On) {
                            this.toggle_column(column, cx);
                            cx.notify();
                        }
                    });
                }),
            ));
        }
        self.check(
            "ng.finder.options.relative",
            (19., 514.),
            "Use relative dates",
            look.relative,
            |this, on, cx| this.change_list_look(|look| look.relative = on, cx),
        );
        self.check(
            "ng.finder.options.sizes",
            (19., 534.),
            "Calculate all sizes",
            look.sizes,
            |this, on, cx| this.change_list_look(|look| look.sizes = on, cx),
        );
        self.previews(554.);
    }

    fn column(&mut self) {
        let look = self.state.column_look();
        self.text_size((76.5, 153.), look.text, |this, text, cx| {
            this.change_column_look(|look| look.text = text, cx)
        });
        self.check(
            "ng.finder.options.icons",
            (19., 181.),
            "Show icons",
            look.icons,
            |this, on, cx| this.change_column_look(|look| look.icons = on, cx),
        );
        self.previews(201.);
        self.preview_column(221.);
        self.check(
            "ng.finder.options.fit",
            (19., 241.),
            "Resize columns to fit filenames",
            look.fit,
            |this, on, cx| this.change_column_look(|look| look.fit = on, cx),
        );
    }

    fn gallery(&mut self) {
        name_thumbnails();
        let look = self.state.gallery_look();
        self.put(label(
            "ng.finder.options.thumb.label",
            (19., 125., 85.),
            "Thumbnail size:",
        ));
        let change: Change<f32> =
            |this, thumb, cx| this.change_gallery_look(|look| look.thumb = thumb, cx);
        for (index, (side, name, _, (x, y), radio)) in THUMBS.into_iter().enumerate() {
            let id = ("ng.finder.options.thumb", index);
            self.pick(
                name,
                (x, y),
                Some(native_image(id, name).into_any_element()),
                (side, change),
            );
            self.radio(
                ("ng.finder.options.thumb.size", index),
                (radio, 207.),
                "",
                (side, look.thumb == side),
                change,
            );
        }
        self.preview_column(238.);
        self.previews(258.);
        self.check(
            "ng.finder.options.filename",
            (19., 278.),
            "Show filename",
            look.filename,
            |this, on, cx| this.change_gallery_look(|look| look.filename = on, cx),
        );
    }

    fn icon(&mut self) {
        let options = self.state.icon_options();
        let placed = |doc: Option<AnyElement>, (x, y)| doc.map(|doc| at(x, y, doc));
        self.put(label(
            "ng.finder.options.icon.label",
            (19., 153., 52.),
            "Icon size:",
        ));
        self.put(at(
            77.,
            153.,
            div().w(px(40.)).child(
                native_label(
                    "ng.finder.options.icon.points",
                    &format!("{0} \u{d7} {0}", options.size),
                )
                .color(SystemColor::SecondaryLabel),
            ),
        ));
        self.put(placed(
            document("ng.finder.options.icon.small", 16.),
            (20., 172.),
        ));
        let run = self.run(|this, step: f64, cx| {
            this.change_icon_options(|options| options.icon_size = Some(slider_size(step)), cx)
        });
        self.put(at(
            43.,
            170.,
            div().w(px(134.)).child(
                native_slider("ng.finder.options.size", size_slider(options.size))
                    .range(0., SIZE_STEPS)
                    .on_change(move |step: &f64, _, cx| run(*step, cx)),
            ),
        ));
        self.put(placed(
            document("ng.finder.options.icon.large", 32.),
            (184., 164.),
        ));
        self.put(label(
            "ng.finder.options.spacing.label",
            (19., 198., 72.),
            "Grid spacing:",
        ));
        self.put(at(
            20.,
            217.,
            native_symbol(
                "ng.finder.options.spacing.small",
                "square.grid.3x3",
                13.,
                gpui::FontWeight::NORMAL,
            ),
        ));
        let run = self.run(|this, spacing: f64, cx| {
            this.change_icon_options(
                |options| options.grid_spacing = Some(spacing.round() as f32),
                cx,
            )
        });
        self.put(at(
            43.,
            215.,
            div().w(px(134.)).child(
                native_slider("ng.finder.options.spacing", f64::from(options.spacing))
                    .range(1., 100.)
                    .on_change(move |spacing: &f64, _, cx| run(*spacing, cx)),
            ),
        ));
        self.put(at(
            185.,
            210.,
            native_symbol(
                "ng.finder.options.spacing.large",
                "square.grid.2x2",
                26.,
                gpui::FontWeight::NORMAL,
            ),
        ));
        self.text_size((76.5, 257.), options.text, |this, text, cx| {
            this.change_icon_options(|options| options.text_size = Some(text), cx)
        });
        self.put(label(
            "ng.finder.options.label.label",
            (19., 285., 79.),
            "Label position:",
        ));
        let placed: Change<bool> = |this, bottom, cx| {
            this.change_icon_options(|options| options.label_on_bottom = Some(bottom), cx)
        };
        self.radio(
            ("ng.finder.options.label", 1usize),
            (19., 306.),
            "Bottom",
            (true, options.bottom),
            placed,
        );
        self.radio(
            ("ng.finder.options.label", 0usize),
            (83.5, 306.),
            "Right",
            (false, !options.bottom),
            placed,
        );
        self.check(
            "ng.finder.options.info",
            (19., 337.),
            "Show item info",
            options.info,
            |this, on, cx| {
                this.change_icon_options(|options| options.show_item_info = Some(on), cx)
            },
        );
        self.previews(357.);
        self.background(&self.state.icon_background());
    }

    fn background(&mut self, background: &Background) {
        self.put(label(
            "ng.finder.options.background.label",
            (19., 408., 68.),
            "Background:",
        ));
        let chosen: Change<u8> = |this, kind, cx| {
            let background = match (kind, this.icon_background()) {
                (0, _) => Background::Default,
                (1, Background::Color(color)) => Background::Color(color),
                (1, _) => Background::Color([1., 1., 1.]),
                (_, Background::Picture(path)) => Background::Picture(path),
                _ => Background::Picture(String::new()),
            };
            this.change_icon_options(|options| options.background = Some(background), cx)
        };
        let kind = match background {
            Background::Default => 0,
            Background::Color(_) => 1,
            Background::Picture(_) => 2,
        };
        for (index, title) in ["Default", "Color", "Picture"].into_iter().enumerate() {
            self.radio(
                ("ng.finder.options.background", index),
                (19., 425. + 18. * index as f32),
                title,
                (index as u8, kind == index),
                chosen,
            );
        }
        match background {
            Background::Color(color) => {
                let demo = self.demo.clone();
                self.put(at(
                    93.,
                    440.,
                    native_color_well("ng.finder.options.color", color_of(*color))
                        .supports_alpha(false)
                        .on_change(move |color: &Hsla, _, cx| {
                            let rgb = color.to_rgb();
                            demo.update(cx, |this, cx| {
                                this.change_icon_options(
                                    |options| {
                                        options.background =
                                            Some(Background::Color([rgb.r, rgb.g, rgb.b]))
                                    },
                                    cx,
                                )
                            });
                        }),
                ));
            }
            Background::Picture(path) => self.picture_well(path),
            Background::Default => {}
        }
    }

    fn picture_well(&mut self, path: &str) {
        let demo = self.demo.clone();
        let dropped = self.demo.clone();
        let named = PathBuf::from(path)
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
                    let Some(path) = paths.paths().first() else {
                        return;
                    };
                    let path = path.to_string_lossy().into_owned();
                    dropped.update(cx, |this, cx| {
                        this.change_icon_options(
                            |options| options.background = Some(Background::Picture(path)),
                            cx,
                        )
                    });
                })
                .into_any_element(),
        );
    }
}

fn choose_picture(cx: &mut Context<FinderDemo>) {
    // From a task, outside GPUI's borrow of the app, which the modal loop would re-enter.
    cx.spawn(async move |this, cx: &mut AsyncApp| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(path) = panel.URL().and_then(|url| url.path()) else {
            return;
        };
        let path = path.to_string();
        this.update(cx, |this, cx| {
            this.change_icon_options(
                |options| options.background = Some(Background::Picture(path)),
                cx,
            );
            cx.notify();
        })
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
            (19., 35.),
            &format!("Always open in {name} view"),
            opens.is_some(),
            |this, on, _| this.always_open(on),
        );
        let run = build.run(|this, on, _| this.always_browse(on));
        build.put(at(
            29.,
            57.,
            native_checkbox(
                "ng.finder.options.browse",
                opens.is_some_and(|(_, browse)| browse),
            )
            .title(format!("Browse in {name} view"))
            .enabled(opens.is_some())
            .on_change(move |state: &CheckState, _, cx| run(*state == CheckState::On, cx)),
        ));
        match view {
            GALLERY_VIEW => build.sort(88.),
            _ => {
                build.group();
                build.sort(116.);
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
                56.5,
                605.,
                native_button("ng.finder.options.defaults", "Use as Defaults")
                    .width(px(123.))
                    .enabled(prefs::has_own_look(view, here))
                    .on_click(move |_, _, cx| run((), cx)),
            ));
        }
        let children = build.out;

        native_root(
            div()
                .size_full()
                .relative()
                .bg(system_color(SystemColor::WindowBackground))
                .children(children),
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
                size: size(px(PANEL.0), px(PANEL.1)),
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
