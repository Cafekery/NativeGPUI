use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use finder_fs::ds_store::Background;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, ExternalPaths, FontWeight, Hsla, Render, Rgba,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point,
    prelude::*, px, size,
};
use native_gpui::tokens::SystemColor;
use native_gpui::{
    LabelAlign, MenuEntry, NativePopup, NativeSlider, native_button, native_color_well,
    native_popup, native_segmented, native_slider, native_symbol, window_options,
};
use objc2::msg_send;
use objc2_app_kit::{NSWindowStyleMask, NSWindowTitleVisibility};

use crate::app::FinderDemo;
use crate::data::{Column, GROUP_KEYS, SORT_KEYS, SORT_TITLES};
use crate::form::{COMPACT, Flip, Row, Sheet, choose_file};
use crate::frame::ns_window;
use crate::prefs;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};
use crate::views::header::column_title;
use crate::views::icon::{Arrangement, SIZE_STEPS, size_slider, slider_size};

const WIDTH: f32 = 260.;
const SLIDER_ROW: f32 = 46.;
const SLIDER_END: f32 = 20.;
const BUTTON: f32 = 20.;

const TEXT_SIZES: [f32; 7] = [10., 11., 12., 13., 14., 15., 16.];
const LIST_ICONS: [f32; 2] = [16., 32.];
const THUMBNAILS: [(f32, &str); 3] = [(32., "Small"), (48., "Medium"), (64., "Large")];

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

fn arranged_title(arrangement: Arrangement) -> &'static str {
    match arrangement {
        Arrangement::Free => "None",
        Arrangement::Grid => "Snap to Grid",
        Arrangement::By(column) => column_title(column),
    }
}

fn symbol(id: &'static str, name: &str, side: f32) -> AnyElement {
    native_symbol(id, name, side, FontWeight::NORMAL).into_any_element()
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

    fn flip(&self, change: Change<bool>) -> Flip {
        Rc::new(self.run(change))
    }

    fn switch(&self, title: &str, on: bool, change: Change<bool>) -> Row {
        Row::new(COMPACT.switch_row(title, on, Some(self.flip(change))))
    }

    fn popup(&self, title: &str, popup: NativePopup, shown: &str) -> Row {
        let popup = popup
            .bordered(false)
            .control_size(COMPACT.controls())
            .font_size(px(COMPACT.font))
            .width(px(COMPACT.hugging(shown)));
        Row::new(COMPACT.labelled(title, SystemColor::Label, popup))
    }

    fn segmented(
        &self,
        id: &'static str,
        title: &str,
        segments: &[&str],
        selected: usize,
        change: Change<usize>,
    ) -> Row {
        let run = self.run(change);
        let control = native_segmented(id, selected)
            .segments(segments.iter().copied())
            .control_size(COMPACT.controls())
            .on_change(move |at: &usize, _, cx| run(*at, cx));
        Row::new(COMPACT.labelled(title, SystemColor::Label, control))
    }

    fn slider(
        &self,
        title: &str,
        value: Option<String>,
        (least, most): (AnyElement, AnyElement),
        slider: NativeSlider,
        change: Change<f64>,
    ) -> Row {
        let run = self.run(change);
        let slider = slider
            .control_size(COMPACT.controls())
            .on_change(move |value: &f64, _, cx| run(*value, cx));
        let end = |image: AnyElement| {
            div()
                .flex_none()
                .size(px(SLIDER_END))
                .flex()
                .items_center()
                .justify_center()
                .child(image)
        };
        let label = div()
            .h(px(COMPACT.line))
            .relative()
            .child(COMPACT.label("label", title))
            .children(value.map(|value| {
                div().absolute().inset_0().child(
                    COMPACT
                        .label("value", &value)
                        .color(SystemColor::SecondaryLabel)
                        .align(LabelAlign::Right),
                )
            }));
        let track = div()
            .h(px(SLIDER_END))
            .flex()
            .items_center()
            .gap(px(6.))
            .child(end(least))
            .child(div().flex_1().child(slider))
            .child(end(most));
        Row::new(
            div()
                .size_full()
                .px(px(COMPACT.inset))
                .pt(px(6.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(label)
                .child(track),
        )
        .tall(SLIDER_ROW)
    }

    fn text_size(&self, size: f32, change: Change<f32>) -> Row {
        let chosen = TEXT_SIZES
            .iter()
            .position(|at| *at == size.round())
            .unwrap_or(2);
        let run = self.run(change);
        let popup = native_popup("ng.finder.options.text", chosen)
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
        self.popup("Text size", popup, &TEXT_SIZES[chosen].to_string())
    }

    fn group(&self) -> Row {
        let here = self.state.here();
        let keys: Vec<Column> = GROUP_KEYS
            .into_iter()
            .filter(|key| key.offered_at(here))
            .collect();
        let grouped = self
            .state
            .group()
            .and_then(|group| keys.iter().position(|key| *key == group));
        let shown = grouped.map_or("None", |at| column_title(keys[at]));
        let entries = [MenuEntry::item("None"), MenuEntry::separator()]
            .into_iter()
            .chain(keys.iter().map(|key| MenuEntry::item(column_title(*key))));
        let run = self.run(|this, group, cx| this.set_group(group, cx));
        let popup = native_popup("ng.finder.options.group", grouped.map_or(0, |at| at + 2))
            .entries(entries)
            .on_change(move |index: &usize, _, cx| match index.checked_sub(2) {
                None => run(None, cx),
                Some(at) => {
                    if let Some(key) = keys.get(at) {
                        run(Some(*key), cx);
                    }
                }
            });
        self.popup("Group By", popup, shown)
    }

    fn sort(&self) -> Row {
        let arranged = self
            .state
            .arrangement()
            .filter(|_| self.state.view == ICON_VIEW);
        let (popup, shown) = match arranged {
            Some(arranged) => self.arrange_popup(arranged),
            None => {
                let chosen = SORT_KEYS
                    .iter()
                    .position(|key| *key == self.state.sort)
                    .unwrap_or(0);
                let run = self.run(|this, column: Column, cx| {
                    if this.sort != column {
                        this.resort((column, column.first_ascending()), cx);
                    }
                });
                let popup = native_popup("ng.finder.options.sort", chosen)
                    .entries(SORT_TITLES.iter().map(|title| MenuEntry::item(*title)))
                    .on_change(move |index: &usize, _, cx| {
                        if let Some(column) = SORT_KEYS.get(*index).copied() {
                            run(column, cx);
                        }
                    });
                (popup, SORT_TITLES[chosen])
            }
        };
        self.popup("Sort By", popup, shown)
    }

    fn arrange_popup(&self, arranged: Arrangement) -> (NativePopup, &'static str) {
        let entries = ARRANGEMENTS.iter().map(|arrangement| match arrangement {
            None => MenuEntry::separator(),
            Some(arrangement) => MenuEntry::item(arranged_title(*arrangement)),
        });
        let chosen = ARRANGEMENTS
            .iter()
            .position(|arrangement| *arrangement == Some(arranged));
        let run = self.run(|this, arrangement, cx| this.arrange(arrangement, cx));
        let popup = native_popup("ng.finder.options.arrange", chosen.unwrap_or(0))
            .entries(entries)
            .on_change(move |index: &usize, _, cx| {
                if let Some(arrangement) = ARRANGEMENTS.get(*index).copied().flatten() {
                    run(arrangement, cx);
                }
            });
        (popup, arranged_title(arranged))
    }

    fn previews(&self) -> Row {
        self.switch(
            "Icon preview",
            self.state.icon_previews(),
            |this, on, cx| this.set_icon_previews(on, cx),
        )
    }

    fn preview_column(&self) -> Row {
        self.switch("Preview column", self.state.preview, |this, on, _| {
            this.show_preview(on)
        })
    }

    fn icon(&self, sheet: &mut Sheet) {
        let options = self.state.icon_options();
        let size = native_slider("ng.finder.options.size", size_slider(options.size))
            .range(0., SIZE_STEPS)
            .ticks(9);
        let spacing = native_slider("ng.finder.options.spacing", f64::from(options.spacing))
            .range(1., 100.)
            .ticks(8);
        sheet.card(
            "ng.finder.options.sizes",
            Some("Size"),
            vec![
                self.slider(
                    "Icon size",
                    Some(format!("{0} \u{d7} {0}", options.size)),
                    (
                        symbol("ng.finder.options.icon.small", "doc", 10.),
                        symbol("ng.finder.options.icon.large", "doc", 16.),
                    ),
                    size,
                    |this, step, cx| {
                        this.change_icon_options(
                            |options| options.icon_size = Some(slider_size(step)),
                            cx,
                        )
                    },
                ),
                self.slider(
                    "Grid spacing",
                    None,
                    (
                        symbol("ng.finder.options.spacing.small", "square.grid.3x3", 10.),
                        symbol("ng.finder.options.spacing.large", "square.grid.2x2", 16.),
                    ),
                    spacing,
                    |this, spacing, cx| {
                        this.change_icon_options(
                            |options| options.grid_spacing = Some(spacing.round() as f32),
                            cx,
                        )
                    },
                ),
            ],
        );
        sheet.card(
            "ng.finder.options.texts",
            Some("Text"),
            vec![
                self.text_size(options.text, |this, text, cx| {
                    this.change_icon_options(|options| options.text_size = Some(text), cx)
                }),
                self.segmented(
                    "ng.finder.options.label",
                    "Label position",
                    &["Bottom", "Right"],
                    usize::from(!options.bottom),
                    |this, at, cx| {
                        this.change_icon_options(
                            |options| options.label_on_bottom = Some(at == 0),
                            cx,
                        )
                    },
                ),
            ],
        );
        sheet.card(
            "ng.finder.options.show",
            Some("Show"),
            vec![
                self.switch("Item info", options.info, |this, on, cx| {
                    this.change_icon_options(|options| options.show_item_info = Some(on), cx)
                }),
                self.previews(),
            ],
        );
        self.background(sheet);
    }

    fn background(&self, sheet: &mut Sheet) {
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
        let run = self.run(chosen);
        let kinds = native_segmented("ng.finder.options.background", kind)
            .segments(["Default", "Color", "Picture"])
            .control_size(COMPACT.controls())
            .on_change(move |kind: &usize, _, cx| run(*kind, cx));
        let mut rows = vec![Row::new(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(kinds),
        )];
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
                rows.push(Row::new(COMPACT.labelled(
                    "Color",
                    SystemColor::Label,
                    well,
                )));
            }
            Background::Picture(path) => rows.push(self.picture(&path)),
            Background::Default => {}
        }
        sheet.card("ng.finder.options.backgrounds", Some("Background"), rows);
    }

    fn picture(&self, path: &str) -> Row {
        let demo = self.demo.clone();
        let dropped = self.run(set_background);
        let named = Path::new(path).file_name().map_or_else(
            || "None".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let choose = native_button("ng.finder.options.picture.choose", "Choose\u{2026}")
            .control_size(COMPACT.controls())
            .on_click(move |_, _, cx| {
                demo.update(cx, |_, cx| {
                    choose_file(cx, None, |this, path, cx| {
                        let picture = Background::Picture(path.to_string_lossy().into_owned());
                        set_background(this, picture, cx)
                    })
                })
            });
        let value = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div().flex_1().child(
                    COMPACT
                        .label("value", &named)
                        .color(SystemColor::SecondaryLabel)
                        .align(LabelAlign::Right)
                        .truncates(true),
                ),
            )
            .child(choose);
        Row::new(
            div()
                .id("ng.finder.options.picture")
                .size_full()
                .on_drop(move |paths: &ExternalPaths, _, cx| {
                    if let Some(path) = paths.paths().first() {
                        let path = path.to_string_lossy().into_owned();
                        dropped(Background::Picture(path), cx);
                    }
                })
                .child(COMPACT.labelled("Picture", SystemColor::Label, value)),
        )
    }

    fn list(&self, sheet: &mut Sheet) {
        let look = self.state.list_look;
        sheet.card(
            "ng.finder.options.sizes",
            Some("Size"),
            vec![
                self.segmented(
                    "ng.finder.options.icon",
                    "Icon size",
                    &["Small", "Large"],
                    usize::from(look.icon >= 24.),
                    |this, at, cx| {
                        if let Some(icon) = LIST_ICONS.get(at).copied() {
                            this.change_list_look(|look| look.icon = icon, cx);
                        }
                    },
                ),
                self.text_size(look.text, |this, text, cx| {
                    this.change_list_look(|look| look.text = text, cx)
                }),
            ],
        );
        let here = self.state.here();
        let columns = SHOW_COLUMNS
            .iter()
            .map(|&(title, column)| {
                let column = column.filter(|column| column.offered_at(here));
                let change = column.map(|column| -> Flip {
                    let demo = self.demo.clone();
                    Rc::new(move |on, cx| {
                        demo.update(cx, |this, cx| {
                            if shows(this, column) != on {
                                this.toggle_column(column, cx);
                                cx.notify();
                            }
                        })
                    })
                });
                let on = column.is_some_and(|column| shows(self.state, column));
                Row::new(COMPACT.switch_row(title, on, change))
            })
            .collect();
        sheet.card("ng.finder.options.columns", Some("Show Columns"), columns);
        sheet.card(
            "ng.finder.options.figures",
            None,
            vec![
                self.switch("Use relative dates", look.relative, |this, on, cx| {
                    this.change_list_look(|look| look.relative = on, cx)
                }),
                self.switch("Calculate all sizes", look.sizes, |this, on, cx| {
                    this.change_list_look(|look| look.sizes = on, cx)
                }),
            ],
        );
        sheet.card(
            "ng.finder.options.show",
            Some("Show"),
            vec![self.previews()],
        );
    }

    fn column(&self, sheet: &mut Sheet) {
        let look = self.state.column_look;
        sheet.card(
            "ng.finder.options.texts",
            Some("Text"),
            vec![
                self.text_size(look.text, |this, text, cx| {
                    this.change_column_look(|look| look.text = text, cx)
                }),
                self.switch(
                    "Resize columns to fit filenames",
                    look.fit,
                    |this, on, cx| this.change_column_look(|look| look.fit = on, cx),
                ),
            ],
        );
        sheet.card(
            "ng.finder.options.show",
            Some("Show"),
            vec![
                self.switch("Icons", look.icons, |this, on, cx| {
                    this.change_column_look(|look| look.icons = on, cx)
                }),
                self.previews(),
                self.preview_column(),
            ],
        );
    }

    fn gallery(&self, sheet: &mut Sheet) {
        let look = self.state.gallery_look;
        let chosen = THUMBNAILS
            .iter()
            .rposition(|(side, _)| *side <= look.thumb)
            .unwrap_or(0);
        let run =
            self.run(|this, thumb, cx| this.change_gallery_look(|look| look.thumb = thumb, cx));
        let popup = native_popup("ng.finder.options.thumb", chosen)
            .entries(THUMBNAILS.iter().map(|(_, title)| MenuEntry::item(*title)))
            .on_change(move |at: &usize, _, cx| {
                if let Some((thumb, _)) = THUMBNAILS.get(*at) {
                    run(*thumb, cx);
                }
            });
        sheet.card(
            "ng.finder.options.sizes",
            Some("Size"),
            vec![self.popup("Thumbnail size", popup, THUMBNAILS[chosen].1)],
        );
        sheet.card(
            "ng.finder.options.show",
            Some("Show"),
            vec![
                self.preview_column(),
                self.previews(),
                self.switch("Filename", look.filename, |this, on, cx| {
                    this.change_gallery_look(|look| look.filename = on, cx)
                }),
            ],
        );
    }
}

fn sheet(demo: &Entity<FinderDemo>, state: &FinderDemo, width: f32, hair: f32) -> Sheet {
    let build = Build { demo, state };
    let (view, here) = (state.view, state.here());
    let name = view_name(view);
    let opens = prefs::opens_in(here).filter(|(opens, _)| *opens == view);
    let browse = opens
        .is_some()
        .then(|| build.flip(|this, on, _| this.always_browse(on)));
    let mut sheet = Sheet::new(width, hair);
    sheet.card(
        "ng.finder.options.opens",
        None,
        vec![
            build.switch(
                &format!("Always open in {name} view"),
                opens.is_some(),
                |this, on, _| this.always_open(on),
            ),
            Row::new(COMPACT.switch_row(
                &format!("Browse in {name} view"),
                opens.is_some_and(|(_, browse)| browse),
                browse,
            ))
            .nested(),
        ],
    );
    let mut arrange = Vec::new();
    if view != GALLERY_VIEW {
        arrange.push(build.group());
    }
    arrange.push(build.sort());
    sheet.card("ng.finder.options.arranges", None, arrange);
    match view {
        LIST_VIEW => build.list(&mut sheet),
        COLUMN_VIEW => build.column(&mut sheet),
        GALLERY_VIEW => build.gallery(&mut sheet),
        _ => build.icon(&mut sheet),
    }
    if view != COLUMN_VIEW {
        let run = build.run(|this, (), _| prefs::use_as_defaults(this.view, this.here()));
        let button = native_button("ng.finder.options.defaults", "Use as Defaults")
            .control_size(COMPACT.controls())
            .enabled(prefs::has_own_settings(view, here))
            .on_click(move |_, _, cx| run((), cx));
        sheet.button(button, BUTTON);
    }
    sheet
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
        let width = f32::from(window.viewport_size().width);
        sheet(&demo, state, width, 1. / window.scale_factor())
            .into_page(window)
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
    let tall = sheet(demo, demo.read(cx), WIDTH, 1.).height();
    let watched = demo.clone();
    let demo = demo.downgrade();
    let opened = cx.open_window(
        WindowOptions {
            kind: WindowKind::Floating,
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(320.), px(220.)),
                size: size(px(WIDTH), px(tall)),
            })),
            is_resizable: false,
            is_minimizable: false,
            focus: false,
            ..window_options()
        },
        |window, cx| {
            if let Some(panel) = ns_window(window) {
                // A switch draws its accent only in the key window, so a click has to make this key.
                let _: () = unsafe { msg_send![&*panel, setFloatingPanel: true] };
                panel.setStyleMask(
                    panel.styleMask()
                        | NSWindowStyleMask::UtilityWindow
                        | NSWindowStyleMask::NonactivatingPanel,
                );
                panel.setTitleVisibility(NSWindowTitleVisibility::Visible);
                crate::chrome::toolbar::system_corners_of(&panel);
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
