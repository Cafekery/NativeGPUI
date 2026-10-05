use std::ops::Range;

use finder_fs::tags::Tag;
use gpui::{
    AnyElement, ClickEvent, Context, Div, IntoElement, ParentElement, Render, ScrollStrategy,
    SharedString, Styled, Window, div, prelude::*, px, uniform_list,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    CheckState, ControlSize, NativeLabel, SegmentStyle, TrackingMode, native_checkbox, native_rule,
    native_segmented, native_switch, native_text_field,
};

use super::{Settings, at, global_on, label, lines};
use crate::chrome::sidebar;
use crate::tags::finder_color_rgb;

const ROW_H: f32 = 20.;
const FAVOURITES_X: f32 = 77.;
const FAVOURITE_W: f32 = 32.;
const FAVOURITES_PER_ROW: usize = 7;

/// A tag dragged from the table, or from the favourites at this index.
#[derive(Clone)]
pub(super) struct DraggedTag {
    name: String,
    color: u8,
    favourite: Option<usize>,
}

impl Render for DraggedTag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let preview = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(dot(self.color));
        match self.favourite {
            Some(_) => preview,
            None => preview.child(
                div()
                    .text_size(tokens::system_font_size())
                    .text_color(system_color(SystemColor::Label))
                    .child(SharedString::from(self.name.clone())),
            ),
        }
    }
}

fn dot(color: u8) -> Div {
    let dot = div().size(px(12.)).rounded_full();
    match finder_color_rgb(color) {
        Some((r, g, b)) => dot.bg(gpui::rgb(u32::from_be_bytes([0, r, g, b]))),
        None => dot
            .border_1()
            .border_color(system_color(SystemColor::SecondaryLabel)),
    }
}

fn small(text: &str) -> NativeLabel {
    label(text).font_size(tokens::small_system_font_size())
}

fn names(tags: &[Tag]) -> Vec<String> {
    tags.iter().map(|tag| tag.name.clone()).collect()
}

fn borrowed(names: &[String]) -> Vec<&str> {
    names.iter().map(String::as_str).collect()
}

impl Settings {
    pub(super) fn tags(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let listed = crate::tags::all();
        let gone = |picked: &String| !listed.iter().any(|tag| &tag.name == picked);
        if self.tag_picked.as_ref().is_some_and(gone) {
            self.tag_picked = None;
            self.tag_draft = None;
        }
        let rows = uniform_list(
            "ng.finder.settings.tags",
            listed.len(),
            cx.processor(|this, range: Range<usize>, _, cx| {
                let listed = crate::tags::all();
                range
                    .map(|at| this.tag_row(&listed[at], at, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.tag_scroll)
        .size_full();
        div()
            .size_full()
            .child(
                at(26., 108.)
                    .w(px(300.))
                    .child(label("Show these tags in the sidebar:")),
            )
            .child(
                at(20., 126.)
                    .w(px(337.))
                    .h(px(251.))
                    .border_1()
                    .border_color(system_color(SystemColor::Separator))
                    .child(rows),
            )
            .child(
                at(20., 386.).child(
                    native_segmented("ng.finder.settings.tag_edit", 0)
                        .segments(["Add", "Remove"])
                        .symbols(["plus", "minus"])
                        .tracking_mode(TrackingMode::Momentary)
                        .style(SegmentStyle::SmallSquare)
                        .segment_enabled(1, self.tag_picked.is_some())
                        .on_change(cx.listener(|this, segment: &usize, _, cx| match segment {
                            0 => this.add_tag(cx),
                            _ => this.delete_tag(cx),
                        })),
                ),
            )
            .child(lines(
                26.,
                420.,
                14.,
                &[
                    "Drag your favorite tags to the area below to make them",
                    "available for quick use in Finder menus.",
                ],
                small,
            ))
            .child(self.favourites(cx))
            .child(
                at(20., 535.)
                    .w(px(336.))
                    .h(px(36.))
                    .rounded(px(10.))
                    .bg(system_color(SystemColor::QuaternarySystemFill))
                    .child(
                        div()
                            .absolute()
                            .left(px(6.))
                            .top(px(10.))
                            .w(px(247.))
                            .child(label("Tint folders based on tags")),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(259.))
                            .top(px(6.))
                            .w(px(68.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                native_switch(
                                    "ng.finder.settings.tint",
                                    !global_on("AppleDisableTagBasedIconTinting"),
                                )
                                .enabled(false),
                            ),
                    ),
            )
            .on_drop(cx.listener(|this, dragged: &DraggedTag, _, cx| {
                if let Some(index) = dragged.favourite {
                    this.unfavourite(index, cx);
                }
            }))
            .into_any_element()
    }

    fn tag_row(&mut self, tag: &Tag, at: usize, cx: &mut Context<Self>) -> AnyElement {
        let id = crate::data::intern(&format!("tag:{}", tag.name));
        let picked = self.tag_picked.as_deref() == Some(tag.name.as_str());
        let name = div().absolute().left(px(32.)).top(px(2.)).w(px(269.));
        let name = match self.tag_draft.as_ref().filter(|_| picked) {
            Some(draft) => {
                let mut field = native_text_field(("ng.finder.settings.tag_name", at), draft)
                    .stretch()
                    .control_size(ControlSize::Small)
                    .on_change(cx.listener(|this, text: &String, _, cx| {
                        this.tag_draft = Some(text.clone());
                        cx.notify();
                    }))
                    .on_submit(cx.listener(|this, _: &String, _, cx| {
                        this.finish_naming(cx);
                    }))
                    .on_cancel(cx.listener(|this, _: &String, _, cx| {
                        this.tag_draft = None;
                        cx.notify();
                    }));
                if std::mem::take(&mut self.tag_fresh) {
                    field = field.editing(0..draft.chars().count());
                }
                name.child(field)
            }
            None => name.child(label(&tag.name).color(if picked {
                SystemColor::AlternateSelectedControlText
            } else {
                SystemColor::Label
            })),
        };
        let dragged = DraggedTag {
            name: tag.name.clone(),
            color: tag.color,
            favourite: None,
        };
        let clicked = tag.name.clone();
        div()
            .id(("ng.finder.settings.tag", at))
            .relative()
            .h(px(ROW_H))
            .when(at % 2 == 1, |row| {
                row.bg(system_color(SystemColor::QuinaryLabel))
            })
            .when(picked, |row| {
                row.child(
                    div().absolute().inset_0().child(
                        native_rule(("ng.finder.settings.tag_pick", at))
                            .color(SystemColor::SelectedContentBackground),
                    ),
                )
            })
            .child(
                div()
                    .absolute()
                    .left(px(14.))
                    .top(px(4.))
                    .child(dot(tag.color)),
            )
            .child(name)
            .child(
                div().absolute().left(px(306.)).top(px(3.)).child(
                    native_checkbox(("ng.finder.settings.tag_shown", at), false)
                        .state(sidebar::tag_shown(id))
                        .allows_mixed(true)
                        .control_size(ControlSize::Small)
                        .on_change(move |state: &CheckState, window, cx| {
                            sidebar::show_tag_row(id, *state != CheckState::Off);
                            sidebar::reload_sidebars(cx);
                            window.refresh();
                        }),
                ),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if this.tag_picked.as_ref() != Some(&clicked) {
                    this.finish_naming(cx);
                    this.tag_picked = Some(clicked.clone());
                    cx.notify();
                }
            }))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedTag>(|style, _, _, _| {
                style.bg(system_color(
                    SystemColor::UnemphasizedSelectedContentBackground,
                ))
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedTag, _, cx| {
                this.drop_on_row(dragged, at, cx);
            }))
            .into_any_element()
    }

    fn favourites(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let favourites = crate::tags::tags();
        at(61., 455.)
            .id("ng.finder.settings.favourites")
            .w(px(255.))
            .h(px(70.))
            .rounded(px(6.))
            .border_1()
            .border_color(system_color(SystemColor::TertiaryLabel))
            .child(
                div()
                    .absolute()
                    .left(px(19.))
                    .top(px(5.))
                    .w(px(200.))
                    .child(label("Tags…").color(SystemColor::SecondaryLabel)),
            )
            .children(favourites.iter().enumerate().map(|(index, tag)| {
                let (column, row) = (index % FAVOURITES_PER_ROW, index / FAVOURITES_PER_ROW);
                div()
                    .id(("ng.finder.settings.favourite", index))
                    .absolute()
                    .left(px(25. + FAVOURITE_W * column as f32))
                    .top(px(28. + 14. * row as f32))
                    .child(dot(tag.color))
                    .on_drag(
                        DraggedTag {
                            name: tag.name.clone(),
                            color: tag.color,
                            favourite: Some(index),
                        },
                        |dragged, _, _, cx| cx.new(|_| dragged.clone()),
                    )
            }))
            .child(
                div().absolute().left_0().right_0().top(px(47.)).child(
                    small("Favorite Tags")
                        .color(SystemColor::SecondaryLabel)
                        .align(native_gpui::LabelAlign::Center),
                ),
            )
            .on_drop(cx.listener(|this, dragged: &DraggedTag, window, cx| {
                this.drop_on_favourites(dragged, window, cx);
            }))
    }

    fn add_tag(&mut self, cx: &mut Context<Self>) {
        self.finish_naming(cx);
        let listed = crate::tags::all();
        let Some(name) = (1..)
            .map(|n| match n {
                1 => "untitled".to_owned(),
                n => format!("untitled {n}"),
            })
            .find(|name| !listed.iter().any(|tag| &tag.name == name))
        else {
            return;
        };
        crate::tags::add(&name);
        self.tag_scroll.scroll_to_item(
            crate::tags::all().len().saturating_sub(1),
            ScrollStrategy::Top,
        );
        self.tag_picked = Some(name.clone());
        self.tag_draft = Some(name);
        self.tag_fresh = true;
        sidebar::reload_sidebars(cx);
        cx.notify();
    }

    fn delete_tag(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.tag_picked.take() else {
            return;
        };
        self.tag_draft = None;
        crate::tags::delete(&name);
        sidebar::reload_sidebars(cx);
        cx.notify();
    }

    fn finish_naming(&mut self, cx: &mut Context<Self>) {
        let (Some(old), Some(draft)) = (self.tag_picked.clone(), self.tag_draft.take()) else {
            return;
        };
        let new = draft.trim();
        let taken = crate::tags::all().iter().any(|tag| tag.name == new);
        if !new.is_empty() && !taken {
            crate::tags::rename(&old, new);
            self.tag_picked = Some(new.to_owned());
            sidebar::reload_sidebars(cx);
        }
        cx.notify();
    }

    fn drop_on_row(&mut self, dragged: &DraggedTag, at: usize, cx: &mut Context<Self>) {
        if let Some(index) = dragged.favourite {
            self.unfavourite(index, cx);
            return;
        }
        let mut order = names(&crate::tags::all());
        let Some(from) = order.iter().position(|name| *name == dragged.name) else {
            return;
        };
        let moved = order.remove(from);
        order.insert(at.min(order.len()), moved);
        crate::tags::set_order(&borrowed(&order));
        sidebar::reload_sidebars(cx);
        cx.notify();
    }

    fn drop_on_favourites(
        &mut self,
        dragged: &DraggedTag,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let mut favourites = names(&crate::tags::tags());
        let x = f32::from(window.mouse_position().x) - FAVOURITES_X;
        let mut slot = (x / FAVOURITE_W).round().max(0.) as usize;
        match dragged.favourite {
            Some(from) => {
                let moved = favourites.remove(from);
                if slot > from {
                    slot -= 1;
                }
                favourites.insert(slot.min(favourites.len()), moved);
            }
            None if favourites.contains(&dragged.name) => return,
            None => favourites.insert(slot.min(favourites.len()), dragged.name.clone()),
        }
        crate::tags::set_favourites(&borrowed(&favourites));
        cx.notify();
    }

    fn unfavourite(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut favourites = names(&crate::tags::tags());
        if index < favourites.len() {
            favourites.remove(index);
            crate::tags::set_favourites(&borrowed(&favourites));
            cx.notify();
        }
    }
}
