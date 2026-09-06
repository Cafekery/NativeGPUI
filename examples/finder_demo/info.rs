//! Finder's ⌘I window: one per item, with the General block it opens on. A big selection
//! combines into a single summary window instead of one per item.

use gpui::{
    App, Bounds, Context, Entity, FontWeight, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::window_options;

use std::path::Path;

use crate::art::row_art;
use crate::data::{Line, byte_string, day_string};

const WIDTH: f32 = 265.;
const HEIGHT: f32 = 541.;
const LABEL_W: f32 = 74.;

pub(crate) struct Info {
    line: Line,
    where_: SharedString,
}

impl Info {
    fn field(label: &str, value: SharedString) -> impl IntoElement {
        div()
            .flex()
            .items_start()
            .gap(px(6.))
            .child(
                div()
                    .w(px(LABEL_W))
                    .flex()
                    .justify_end()
                    .text_size(tokens::small_system_font_size())
                    .text_color(system_color(SystemColor::Label))
                    .child(SharedString::from(format!("{label}:"))),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(tokens::small_system_font_size())
                    .text_color(system_color(SystemColor::Label))
                    .child(value),
            )
    }

    fn section(title: &str, open: bool) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(tokens::small_system_font_size())
            .text_color(system_color(SystemColor::Label))
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

impl Render for Info {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = &self.line.row;
        let scale = window.scale_factor();
        let size_text: SharedString = row.size.clone();
        div()
            .size_full()
            .bg(system_color(SystemColor::WindowBackground))
            .pt(px(34.))
            .px(px(12.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
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
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .text_size(tokens::small_system_font_size())
                                            .text_color(system_color(SystemColor::Label))
                                            .child(row.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(tokens::small_system_font_size())
                                            .text_color(system_color(SystemColor::Label))
                                            .child(size_text.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(tokens::small_system_font_size())
                                    .text_color(system_color(SystemColor::SecondaryLabel))
                                    .child(SharedString::from(format!(
                                        "Modified: {}",
                                        day_string(row.modified_at)
                                    ))),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(tokens::small_system_font_size())
                    .text_color(system_color(SystemColor::TertiaryLabel))
                    .child("Add Tags…"),
            )
            .child(Self::section("General", true))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(Self::field("Kind", row.kind.clone()))
                    .child(Self::field("Size", size_text))
                    .child(Self::field("Where", self.where_.clone()))
                    .child(Self::field("Created", day_string(row.created_at).into()))
                    .child(Self::field("Modified", day_string(row.modified_at).into())),
            )
            .child(Self::section("More Info", false))
            .child(Self::section("Name & Extension", false))
            .child(Self::section("Comments", false))
            .child(Self::section("Preview", false))
            .child(Self::section("Sharing & Permissions", false))
    }
}

/// A selection past this size gets one combined window; Finder needs ⌃⌘I for that, ours needs
/// only ⌘I once there are enough items that N separate windows would be a mess.
const COMBINE_OVER: usize = 3;

/// Finder opens one window per chosen item, cascaded down the screen; ours combines a selection
/// bigger than [`COMBINE_OVER`] into a single summary window instead. `at` names the folder to
/// open on in place of the selection, as the path bar's crumbs do.
pub(crate) fn open(demo: &Entity<crate::FinderDemo>, at: Option<&'static str>, cx: &mut App) {
    let (lines, path) = {
        let state = demo.read(cx);
        let lines = match at {
            Some(id) => crate::data::path_line(id).into_iter().collect(),
            None => state
                .focused_lines()
                .iter()
                .filter(|line| state.selected.contains(&line.key))
                .cloned()
                .collect::<Vec<_>>(),
        };
        let holder = at
            .and_then(|id| Path::new(id).parent())
            .map(|parent| crate::data::folder_of(crate::data::intern_path(parent)));
        let crumbs = match &holder {
            Some(folder) => Some(folder.crumbs.as_slice()),
            None => state.folder().map(|folder| folder.crumbs.as_slice()),
        };
        (
            lines,
            SharedString::from(
                crumbs
                    .map(|crumbs| crumbs.join(" \u{203a} "))
                    .unwrap_or_default(),
            ),
        )
    };
    if lines.len() > COMBINE_OVER {
        open_combined(lines, path, cx);
        return;
    }
    for (step, line) in lines.into_iter().enumerate() {
        let where_ = path.clone();
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(60. + step as f32 * 24.), px(80. + step as f32 * 24.)),
                    size: size(px(WIDTH), px(HEIGHT)),
                })),
                ..window_options()
            },
            move |window, cx| {
                window.set_window_title(&format!("{} Info", line.row.name));
                cx.new(|_| Info { line, where_ })
            },
        );
    }
}

/// One window for the whole selection: combined size, the kinds present, and where they are.
struct Combined {
    lines: Vec<Line>,
    where_: SharedString,
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
        let kind_text: SharedString = kinds.join(", ").into();
        div()
            .size_full()
            .bg(system_color(SystemColor::WindowBackground))
            .pt(px(34.))
            .px(px(12.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(tokens::small_system_font_size())
                    .text_color(system_color(SystemColor::Label))
                    .child(SharedString::from(format!("{count} Items"))),
            )
            .child(Info::section("General", true))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(Info::field("Kind", kind_text))
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
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(tokens::small_system_font_size())
                                    .text_color(system_color(SystemColor::Label))
                                    .child(line.row.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(tokens::small_system_font_size())
                                    .text_color(system_color(SystemColor::SecondaryLabel))
                                    .child(line.row.size.clone()),
                            )
                    })),
            )
    }
}

fn open_combined(lines: Vec<Line>, where_: SharedString, cx: &mut App) {
    let count = lines.len();
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(60.), px(80.)),
                size: size(px(WIDTH), px(HEIGHT)),
            })),
            ..window_options()
        },
        move |window, cx| {
            window.set_window_title(&format!("{count} Items Info"));
            cx.new(|_| Combined { lines, where_ })
        },
    );
}
