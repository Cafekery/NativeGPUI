use gpui::{
    AnyElement, ClickEvent, Context, DragMoveEvent, ElementId, FontWeight, Window, div, prelude::*,
    px,
};
use native_gpui::tokens::{self, FileType, SystemColor};
use native_gpui::{native_label, native_raster, native_rule, native_symbol};

use crate::app::FinderDemo;
use crate::data::{Face, folder_artwork};
use crate::dragdrop::RowDrag;
use crate::menus;
use crate::palette::Palette;
use crate::ui::text_width;

pub(crate) const PATH_H: f32 = 28.;

const CHEVRON_W: f32 = 8.;

const CRUMB_INSET: f32 = 16.;

const CRUMB_GAP: f32 = 5.;

const CRUMB_ICON: f32 = 13.;

const DROP_RADIUS: f32 = 7.;

const HAIRLINE: f32 = 0.5;

fn is_alt_click(event: &ClickEvent) -> bool {
    matches!(event, ClickEvent::Mouse(mouse) if mouse.up.modifiers.alt)
}

fn crumb_art(id: impl Into<ElementId>, folder: Option<FileType<'static>>) -> AnyElement {
    let (kind, face) = match folder {
        None => (FileType::Path("/"), Face::Volume),
        Some(kind) => (kind, Face::Folder),
    };
    let raster = crate::icons::icon(kind, CRUMB_ICON)
        .or_else(|| crate::icons::icon(face.stand_in(), CRUMB_ICON));
    div()
        .size(px(CRUMB_ICON))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .children(raster.map(|raster| native_raster(id, raster)))
        .into_any_element()
}

impl FinderDemo {
    pub(crate) fn path_bar_h(&self) -> f32 {
        if self.path_bar_shown { PATH_H } else { 0. }
    }

    pub(crate) fn path_bar(
        &self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let here = self.folders.get(self.target());
        let crumbs: &[&str] = here.map_or(&[][..], |folder| folder.crumbs.as_slice());
        let listed = here.is_some_and(|folder| !folder.rows.is_empty());
        let chain = self.crumb_ids();
        let widths = self.crumb_widths(crumbs, window);
        let lit = self.crumb_drop.filter(|_| cx.has_active_drag());
        let mut trail = div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(CRUMB_GAP))
            .pl(px(CRUMB_INSET))
            .pr(px(CRUMB_INSET));

        for (index, crumb) in crumbs.iter().enumerate() {
            let target = index
                .checked_sub(crumbs.len() - chain.len())
                .and_then(|step| chain.get(step).copied());
            let art =
                (index > 0 || target.is_some_and(|path| crate::data::is_trash(path))).then(|| {
                    target.map_or(FileType::Folder, |path| {
                        folder_artwork(path, index + 1 < crumbs.len() || listed)
                    })
                });
            let mut segment = div()
                .id(("ng.finder.crumb", index))
                .h_full()
                .flex()
                .items_center()
                .min_w(px(CRUMB_ICON))
                .gap(px(CRUMB_GAP))
                .child(crumb_art(("ng.finder.crumb.art", index), art));
            if let Some(width) = widths[index] {
                let dropping = lit == Some(index);
                segment = segment.child(
                    div()
                        .relative()
                        .min_w(px(0.))
                        .w(px(width))
                        .when(dropping, |name| {
                            name.child(
                                div().absolute().inset_0().child(
                                    native_rule(("ng.finder.crumb.drop", index))
                                        .tint(palette.selection)
                                        .corner_radius(DROP_RADIUS),
                                ),
                            )
                        })
                        .child(
                            native_label(("ng.finder.crumb.name", index), crumb)
                                .font_size(tokens::small_system_font_size())
                                .color(if dropping {
                                    SystemColor::AlternateSelectedControlText
                                } else {
                                    SystemColor::SecondaryLabel
                                }),
                        ),
                );
            }
            if index + 1 < crumbs.len() {
                segment = segment.child(
                    div().flex_none().mr(px(1.)).child(
                        native_symbol(
                            ("ng.finder.crumb.chevron", index),
                            "chevron.right",
                            7.,
                            FontWeight::SEMIBOLD,
                        )
                        .tint(SystemColor::SecondaryLabel),
                    ),
                );
            }
            let segment = segment.when_some(target, |crumb_row, id| {
                crumb_row
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if is_alt_click(event) {
                            crate::open_finder_window(id, 1, cx);
                            window.remove_window();
                        } else {
                            this.go(id, window, cx);
                        }
                    }))
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<RowDrag>, _, cx| {
                            let over = event.bounds.contains(&event.event.position);
                            let lit = match (over, this.crumb_drop == Some(index)) {
                                (true, _) => Some(index),
                                (false, true) => None,
                                (false, false) => return,
                            };
                            if this.crumb_drop != lit {
                                this.crumb_drop = lit;
                                cx.notify();
                            }
                        },
                    ))
                    .on_drop(
                        cx.listener(move |this, grab: &RowDrag, window: &mut Window, cx| {
                            this.crumb_drop = None;
                            let keys = this.drag_keys(&grab.key);
                            this.drop_into(id, &keys, window, cx);
                            cx.notify();
                        }),
                    )
            });
            trail = trail.child(match target {
                Some(id) => menus::attach(
                    ("ng.finder.crumb.menu", index),
                    menus::crumb(id, crumb),
                    cx.entity().downgrade(),
                    segment,
                )
                .into_any_element(),
                None => segment.into_any_element(),
            });
        }

        div()
            .h(px(PATH_H))
            .flex_none()
            .relative()
            .block_mouse_except_scroll()
            // Finder's bar is bare window background under a half-point separator.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(HAIRLINE))
                    .child(native_rule("ng.finder.path.hairline")),
            )
            .child(trail)
            .into_any_element()
    }

    fn crumb_widths(&self, crumbs: &[&str], window: &Window) -> Vec<Option<f32>> {
        let size = tokens::small_system_font_size();
        // An NSTextField label insets its text 2pt a side.
        let mut widths: Vec<Option<f32>> = crumbs
            .iter()
            .map(|crumb| Some(text_width(window, crumb.to_string(), size).ceil() + 4.))
            .collect();
        let room = self.list_room - self.pane_w() - 2. * CRUMB_INSET;
        let trail = |widths: &[Option<f32>]| {
            let icons = crumbs.len() as f32 * (CRUMB_ICON + CRUMB_GAP);
            let chevrons = (crumbs.len().saturating_sub(1)) as f32 * (CHEVRON_W + CRUMB_GAP + 1.);
            let names: f32 = widths.iter().flatten().map(|width| width + CRUMB_GAP).sum();
            icons + chevrons + names
        };
        for index in 0..widths.len().saturating_sub(1) {
            if trail(&widths) <= room {
                break;
            }
            widths[index] = None;
        }
        widths
    }
}
