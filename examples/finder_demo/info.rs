use std::path::Path;

use gpui::{
    App, Bounds, Context, Div, Entity, FontWeight, Render, SharedString, Window, WindowBounds,
    WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::window_options;

use crate::art::row_art;
use crate::data::{
    Line, Row, VolumeInfo, byte_string, day_string, folder_of, intern_path, volume_at,
};

pub(crate) const WIDTH: f32 = 265.;
pub(crate) const HEIGHT: f32 = 541.;
const LABEL_W: f32 = 74.;

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
    pub(crate) line: Line,
    pub(crate) where_: SharedString,
}

impl Info {
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

impl Render for Info {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = &self.line.row;
        let scale = window.scale_factor();
        let fields = match volume_at(&row.path) {
            Some(volume) => volume_fields(&volume, row),
            None => vec![
                ("Kind", row.kind.clone()),
                ("Size", row.size.clone()),
                ("Where", self.where_.clone()),
                ("Created", day_string(row.created_at).into()),
                ("Modified", day_string(row.modified_at).into()),
            ],
        };
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
                                    .child(small(SystemColor::Label).child(row.size.clone())),
                            )
                            .child(small(SystemColor::SecondaryLabel).child(SharedString::from(
                                format!("Modified: {}", day_string(row.modified_at)),
                            ))),
                    ),
            )
            .child(small(SystemColor::TertiaryLabel).child("Add Tags…"))
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
            .child(Self::section("Sharing & Permissions", false))
    }
}

const CRUMB_SEPARATOR: &str = " \u{203a} ";

pub(crate) fn where_of(id: &str) -> Option<String> {
    let parent = Path::new(id).parent()?;
    Some(folder_of(intern_path(parent)).crumbs.join(CRUMB_SEPARATOR))
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
        let at = at.or_else(|| chosen.is_empty().then(|| state.here()));
        let lines = match at {
            Some(id) => crate::data::path_line(id).into_iter().collect(),
            None => chosen,
        };
        let where_ = at.and_then(where_of).or_else(|| {
            state
                .folder()
                .map(|folder| folder.crumbs.join(CRUMB_SEPARATOR))
        });
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
                cx.new(|_| Info { line, where_ })
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
                            .child(small(SystemColor::SecondaryLabel).child(line.row.size.clone()))
                    })),
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
