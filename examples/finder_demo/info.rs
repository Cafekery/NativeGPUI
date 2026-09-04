//! Finder's ⌘I window: one per item, with the General block it opens on.

use gpui::{
    App, Bounds, Context, Entity, FontWeight, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::window_options;

use crate::data::{Line, byte_string, day_string};
use crate::ui::artwork;

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
            .children(crate::ui::small_glyph(
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
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = &self.line.row;
        let size_text: SharedString = match row.opens {
            Some(_) => "--".into(),
            None => byte_string(row.bytes).into(),
        };
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
                    .child(artwork(row, 32.))
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
                    .child(Self::field("Kind", row.face.kind().into()))
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

/// Finder opens one window per chosen item, cascaded down the screen.
pub(crate) fn open(demo: &Entity<crate::FinderDemo>, cx: &mut App) {
    let (lines, path) = {
        let state = demo.read(cx);
        (
            state
                .focused_lines()
                .into_iter()
                .filter(|line| state.selected.contains(&line.key))
                .collect::<Vec<_>>(),
            SharedString::from(
                state
                    .folder()
                    .map(|folder| folder.crumbs.join(" \u{203a} "))
                    .unwrap_or_default(),
            ),
        )
    };
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
