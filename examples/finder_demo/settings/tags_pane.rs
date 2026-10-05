use std::ops::Range;

use finder_fs::tags::Tag;
use gpui::{
    AnyElement, ClickEvent, Div, ScrollStrategy, UniformListScrollHandle, Window, div, prelude::*,
    px, uniform_list,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    CheckState, ControlSize, LabelAlign, SegmentStyle, TrackingMode, native_checkbox, native_rule,
    native_segmented, native_switch, native_text_field,
};

use super::{Settings, at, global_on, heading, label, lines, placed, small};
use crate::chrome::sidebar;
use crate::tags::finder_color_rgb;

const ROW_H: f32 = 20.;
const FAVOURITES_X: f32 = 77.;
const FAVOURITE_W: f32 = 32.;
const FAVOURITES_PER_ROW: usize = 7;

#[derive(Default)]
pub(super) struct TagList {
    picked: Option<String>,
    draft: Option<String>,
    fresh: bool,
    scroll: UniformListScrollHandle,
}

#[derive(Clone)]
struct DraggedTag {
    tag: Tag,
    favourite: Option<usize>,
}

impl Render for DraggedTag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(dot(self.tag.color))
            .when(self.favourite.is_none(), |preview| {
                preview.child(
                    div()
                        .text_size(tokens::system_font_size())
                        .text_color(system_color(SystemColor::Label))
                        .child(self.tag.name.clone()),
                )
            })
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

fn names(tags: &[Tag]) -> Vec<&str> {
    tags.iter().map(|tag| tag.name.as_str()).collect()
}

impl Settings {
    pub(super) fn tags(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let listed = crate::tags::all();
        let gone = |picked: &String| !listed.iter().any(|tag| &tag.name == picked);
        if self.tag_list.picked.as_ref().is_some_and(gone) {
            self.tag_list.picked = None;
            self.tag_list.draft = None;
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
        .track_scroll(&self.tag_list.scroll)
        .size_full();
        div()
            .size_full()
            .child(heading(26., 108., "Show these tags in the sidebar:"))
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
                        .segment_enabled(1, self.tag_list.picked.is_some())
                        .on_change(cx.listener(|this, segment: &usize, _, cx| match segment {
                            0 => this.add_tag(cx),
                            _ => this.delete_tag(cx),
                        })),
                ),
            )
            .children(lines(
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
                        placed(6., 10.)
                            .w(px(247.))
                            .child(label("Tint folders based on tags")),
                    )
                    .child(
                        placed(259., 6.)
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
        let picked = self.tag_list.picked.as_deref() == Some(tag.name.as_str());
        let name = placed(32., 2.).w(px(269.));
        let name = match self.tag_list.draft.as_ref().filter(|_| picked) {
            Some(draft) => name.child(
                native_text_field(("ng.finder.settings.tag_name", at), draft)
                    .stretch()
                    .control_size(ControlSize::Small)
                    .on_change(cx.listener(|this, text: &String, _, cx| {
                        this.tag_list.draft = Some(text.clone());
                        cx.notify();
                    }))
                    .on_submit(cx.listener(|this, _: &String, _, cx| this.finish_naming(cx)))
                    .on_cancel(cx.listener(|this, _: &String, _, cx| {
                        this.tag_list.draft = None;
                        cx.notify();
                    }))
                    .when(std::mem::take(&mut self.tag_list.fresh), |field| {
                        field.editing(0..draft.chars().count())
                    }),
            ),
            None => name.child(label(&tag.name).color(if picked {
                SystemColor::AlternateSelectedControlText
            } else {
                SystemColor::Label
            })),
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
            .child(placed(14., 4.).child(dot(tag.color)))
            .child(name)
            .child(
                placed(306., 3.).child(
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
                if this.tag_list.picked.as_ref() != Some(&clicked) {
                    this.finish_naming(cx);
                    this.tag_list.picked = Some(clicked.clone());
                    cx.notify();
                }
            }))
            .on_drag(
                DraggedTag {
                    tag: tag.clone(),
                    favourite: None,
                },
                |dragged, _, _, cx| cx.new(|_| dragged.clone()),
            )
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
        at(61., 455.)
            .id("ng.finder.settings.favourites")
            .w(px(255.))
            .h(px(70.))
            .rounded(px(6.))
            .border_1()
            .border_color(system_color(SystemColor::TertiaryLabel))
            .child(
                placed(19., 5.)
                    .w(px(200.))
                    .child(label("Tags…").color(SystemColor::SecondaryLabel)),
            )
            .children(crate::tags::tags().iter().enumerate().map(|(index, tag)| {
                let (column, row) = (index % FAVOURITES_PER_ROW, index / FAVOURITES_PER_ROW);
                placed(25. + FAVOURITE_W * column as f32, 28. + 14. * row as f32)
                    .id(("ng.finder.settings.favourite", index))
                    .child(dot(tag.color))
                    .on_drag(
                        DraggedTag {
                            tag: tag.clone(),
                            favourite: Some(index),
                        },
                        |dragged, _, _, cx| cx.new(|_| dragged.clone()),
                    )
            }))
            .child(
                div().absolute().left_0().right_0().top(px(47.)).child(
                    small("Favorite Tags")
                        .color(SystemColor::SecondaryLabel)
                        .align(LabelAlign::Center),
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
        self.tag_list.scroll.scroll_to_item(
            crate::tags::all().len().saturating_sub(1),
            ScrollStrategy::Top,
        );
        self.tag_list.picked = Some(name.clone());
        self.tag_list.draft = Some(name);
        self.tag_list.fresh = true;
        sidebar::reload_sidebars(cx);
        cx.notify();
    }

    fn delete_tag(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.tag_list.picked.take() else {
            return;
        };
        self.tag_list.draft = None;
        crate::tags::delete(&name);
        sidebar::reload_sidebars(cx);
        cx.notify();
    }

    fn finish_naming(&mut self, cx: &mut Context<Self>) {
        let list = &mut self.tag_list;
        let (Some(old), Some(draft)) = (list.picked.as_deref(), list.draft.take()) else {
            return;
        };
        let new = draft.trim();
        let taken = crate::tags::all().iter().any(|tag| tag.name == new);
        if !new.is_empty() && !taken {
            crate::tags::rename(old, new);
            list.picked = Some(new.to_owned());
            sidebar::reload_sidebars(cx);
        }
        cx.notify();
    }

    fn drop_on_row(&mut self, dragged: &DraggedTag, at: usize, cx: &mut Context<Self>) {
        if let Some(index) = dragged.favourite {
            return self.unfavourite(index, cx);
        }
        let listed = crate::tags::all();
        let mut order = names(&listed);
        let Some(from) = order.iter().position(|name| *name == dragged.tag.name) else {
            return;
        };
        let moved = order.remove(from);
        order.insert(at.min(order.len()), moved);
        crate::tags::set_order(&order);
        sidebar::reload_sidebars(cx);
        cx.notify();
    }

    fn drop_on_favourites(
        &mut self,
        dragged: &DraggedTag,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let tags = crate::tags::tags();
        let mut favourites = names(&tags);
        let x = f32::from(window.mouse_position().x) - FAVOURITES_X;
        let mut slot = (x / FAVOURITE_W).round().max(0.) as usize;
        let moved = match dragged.favourite {
            Some(from) => {
                if slot > from {
                    slot -= 1;
                }
                favourites.remove(from)
            }
            None if favourites.contains(&dragged.tag.name.as_str()) => return,
            None => dragged.tag.name.as_str(),
        };
        favourites.insert(slot.min(favourites.len()), moved);
        crate::tags::set_favourites(&favourites);
        cx.notify();
    }

    fn unfavourite(&mut self, index: usize, cx: &mut Context<Self>) {
        let tags = crate::tags::tags();
        let mut favourites = names(&tags);
        if index < favourites.len() {
            favourites.remove(index);
            crate::tags::set_favourites(&favourites);
            cx.notify();
        }
    }
}
