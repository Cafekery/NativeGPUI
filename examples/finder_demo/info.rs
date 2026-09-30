use std::any::Any;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use finder_fs::tags::Tag;
use gpui::{
    App, Bounds, Context, Div, Entity, FontWeight, Render, SharedString, Window, WindowBounds,
    WindowOptions, canvas, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{Layer, NativeHost, NativeRead, native_root, window_options, wire};
use objc2::runtime::AnyObject;
use objc2_app_kit::NSTextField;
use objc2_foundation::{NSRect, ns_string};

use crate::art::row_art;
use crate::data::{
    Folder, Line, Row, VolumeInfo, byte_string, day_string, folder_of, intern_path, volume_at,
};
use crate::tag_editor::{tag_field, tags_in};
use crate::tags::{held_tags, retag};

pub(crate) const WIDTH: f32 = 265.;
pub(crate) const HEIGHT: f32 = 541.;
const LABEL_W: f32 = 74.;
const TAGS_H: f32 = 33.;

fn body() -> Div {
    div()
        .size_full()
        .bg(system_color(SystemColor::WindowBackground))
        .pt(px(34.))
        .px(px(12.))
        .pb(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
}

fn small(color: SystemColor) -> Div {
    div()
        .text_size(tokens::small_system_font_size())
        .text_color(system_color(color))
}

fn heading(text: SharedString) -> Div {
    small(SystemColor::Label)
        .font_weight(FontWeight::BOLD)
        .child(text)
}

pub(crate) struct Info {
    line: Line,
    where_: SharedString,
    tags: Option<Rc<[Tag]>>,
}

impl Info {
    pub(crate) fn new(line: Line, where_: SharedString) -> Self {
        let path = Path::new(line.row.path.as_ref());
        let tags = path.is_absolute().then(|| held_tags(path).into());
        Info { line, where_, tags }
    }

    fn field(label: &str, value: SharedString) -> impl IntoElement {
        div()
            .flex()
            .items_start()
            .gap(px(6.))
            .child(
                small(SystemColor::Label)
                    .w(px(LABEL_W))
                    .flex()
                    .justify_end()
                    .child(SharedString::from(format!("{label}:"))),
            )
            .child(small(SystemColor::Label).flex_1().child(value))
    }

    fn section(title: &str, open: bool) -> impl IntoElement {
        small(SystemColor::Label)
            .flex()
            .items_center()
            .gap(px(4.))
            .font_weight(FontWeight::BOLD)
            .children(crate::art::small_glyph(
                if open {
                    "chevron.down"
                } else {
                    "chevron.right"
                },
                9.,
                FontWeight::BOLD,
                SystemColor::SecondaryLabel,
            ))
            .child(SharedString::from(format!("{title}:")))
    }
}

fn volume_fields(volume: &VolumeInfo, row: &Row) -> Vec<(&'static str, SharedString)> {
    vec![
        ("Kind", row.kind.clone()),
        ("Created", day_string(row.created_at).into()),
        ("Modified", day_string(row.modified_at).into()),
        ("Format", volume.format.into()),
        ("Capacity", byte_string(volume.capacity).into()),
        ("Available", byte_string(volume.available).into()),
        (
            "Used",
            byte_string(volume.capacity.saturating_sub(volume.available)).into(),
        ),
    ]
}

fn tags_field(path: SharedString, tags: Rc<[Tag]>) -> impl IntoElement {
    let mut hasher = DefaultHasher::new();
    ("ng.finder.info.tags", path.as_ref()).hash(&mut hasher);
    let key = hasher.finish();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let host = NativeHost::get(window, cx);
            host.place(
                key,
                Layer::Above,
                bounds,
                window.content_mask().bounds,
                |mtm, target| {
                    let field = tag_field(mtm, NSRect::ZERO, &tags);
                    field.setPlaceholderString(Some(ns_string!("Add Tags…")));
                    if let Some(cell) = field.cell() {
                        cell.setSendsActionOnEndEditing(true);
                    }
                    wire(&field, target, key);
                    field
                },
            );
            let read: NativeRead = Rc::new(|sender: &AnyObject| -> Box<dyn Any> {
                Box::new(
                    sender
                        .downcast_ref::<NSTextField>()
                        .map(tags_in)
                        .unwrap_or_default(),
                )
            });
            host.on_event(
                key,
                read,
                Some(Rc::new(move |value, _, cx| {
                    let Ok(tags) = value.downcast::<Vec<Tag>>() else {
                        return;
                    };
                    retag(
                        vec![PathBuf::from(path.as_ref())],
                        move |paths| {
                            let held = finder_fs::tags::of(&paths[0]);
                            if !held
                                .iter()
                                .map(|tag| &tag.name)
                                .eq(tags.iter().map(|tag| &tag.name))
                            {
                                finder_fs::tags::set(&paths[0], &tags).ok();
                            }
                        },
                        cx,
                    );
                })),
            );
        },
    )
    .w_full()
    .h(px(TAGS_H))
}

impl Render for Info {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = &self.line.row;
        let scale = window.scale_factor();
        let about = crate::locations::info_fields(row);
        let place = about.is_some();
        let fields = match (about, volume_at(&row.path)) {
            (Some(about), _) => about,
            (None, Some(volume)) => volume_fields(&volume, row),
            (None, None) => vec![
                ("Kind", row.kind.clone()),
                ("Size", row.size.clone()),
                ("Where", self.where_.clone()),
                ("Created", day_string(row.created_at).into()),
                ("Modified", day_string(row.modified_at).into()),
            ],
        };
        native_root(
            body()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(row_art(row, 32., scale, true))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .justify_between()
                                        .child(heading(row.display.clone()))
                                        .when(!place, |head| {
                                            head.child(
                                                small(SystemColor::Label).child(row.size.clone()),
                                            )
                                        }),
                                )
                                .when(!place, |head| {
                                    head.child(small(SystemColor::SecondaryLabel).child(
                                        SharedString::from(format!(
                                            "Modified: {}",
                                            day_string(row.modified_at)
                                        )),
                                    ))
                                }),
                        ),
                )
                .when_some(self.tags.clone(), |body, tags| {
                    body.child(tags_field(row.path.clone(), tags))
                })
                .child(Self::section("General", true))
                .child(
                    div().flex().flex_col().gap(px(2.)).children(
                        fields
                            .into_iter()
                            .map(|(label, value)| Self::field(label, value)),
                    ),
                )
                .child(Self::section("More Info", false))
                .child(Self::section("Name & Extension", false))
                .child(Self::section("Comments", false))
                .child(Self::section("Preview", false))
                .child(Self::section("Sharing & Permissions", false)),
        )
    }
}

const CRUMB_SEPARATOR: &str = " \u{203a} ";

fn crumb_text(folder: &Folder) -> String {
    let names: Vec<&str> = folder.crumbs.iter().map(|&(name, _)| name).collect();
    names.join(CRUMB_SEPARATOR)
}

pub(crate) fn where_of(id: &str) -> Option<String> {
    let parent = Path::new(id)
        .parent()
        .filter(|parent| parent.is_absolute())?;
    Some(crumb_text(&folder_of(intern_path(parent))))
}

pub(crate) fn open(
    demo: &Entity<crate::app::FinderDemo>,
    at: Option<&'static str>,
    summary: bool,
    cx: &mut App,
) {
    let (lines, where_) = {
        let state = demo.read(cx);
        let chosen = match at {
            Some(_) => Vec::new(),
            None => state.chosen_lines(),
        };
        let at = at.or_else(|| (chosen.is_empty() && !state.results()).then(|| state.here()));
        let lines = match at {
            Some(id) => crate::locations::info_line(id)
                .or_else(|| crate::data::path_line(id))
                .into_iter()
                .collect(),
            None => chosen,
        };
        let where_ = at
            .and_then(where_of)
            .or_else(|| state.folder().map(crumb_text));
        (lines, SharedString::from(where_.unwrap_or_default()))
    };
    if summary && lines.len() > 1 {
        open_combined(lines, where_, cx);
        return;
    }
    for (step, line) in lines.into_iter().enumerate() {
        let where_ = where_.clone();
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(60. + step as f32 * 24.), px(80. + step as f32 * 24.)),
                    size: size(px(WIDTH), px(HEIGHT)),
                })),
                ..window_options()
            },
            move |window, cx| {
                window.set_window_title(&format!("{} Info", line.row.display));
                cx.new(|_| Info::new(line, where_))
            },
        );
    }
}

pub(crate) struct Combined {
    pub(crate) lines: Vec<Line>,
    pub(crate) where_: SharedString,
}

impl Render for Combined {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let count = self.lines.len();
        let total: u64 = self.lines.iter().map(|line| line.row.bytes).sum();
        let mut kinds: Vec<&str> = self
            .lines
            .iter()
            .map(|line| line.row.kind.as_ref())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        native_root(
            body()
                .child(heading(format!("{count} items").into()))
                .child(Info::section("General", true))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(Info::field("Kind", kinds.join(", ").into()))
                        .child(Info::field("Size", byte_string(total).into()))
                        .child(Info::field("Where", self.where_.clone())),
                )
                .child(Info::section("Items", true))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .overflow_hidden()
                        .children(self.lines.iter().map(|line| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(row_art(&line.row, 16., 2., false))
                                .child(
                                    small(SystemColor::Label)
                                        .flex_1()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(line.row.display.clone()),
                                )
                                .child(
                                    small(SystemColor::SecondaryLabel).child(line.row.size.clone()),
                                )
                        })),
                ),
        )
    }
}

fn open_combined(lines: Vec<Line>, where_: SharedString, cx: &mut App) {
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(60.), px(80.)),
                size: size(px(WIDTH), px(HEIGHT)),
            })),
            ..window_options()
        },
        move |window, cx| {
            window.set_window_title("Multiple Item Info");
            cx.new(|_| Combined { lines, where_ })
        },
    );
}
