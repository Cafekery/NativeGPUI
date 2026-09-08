//! Finder's path bar: the trail of folders under the list, each a click, drop and menu target.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Window, div, prelude::*, px};
use native_gpui::native_rule;
use native_gpui::tokens::{self, SystemColor};

use crate::app::FinderDemo;
use crate::art::{place_art, small_glyph};
use crate::chrome::surface::BAR_TINT;
use crate::dragdrop::RowDrag;
use crate::menus;
use crate::palette::Palette;
use crate::ui::text_width;

pub(crate) const PATH_H: f32 = 28.;

const CHEVRON_W: f32 = 8.;

const CRUMB_INSET: f32 = 16.;

const CRUMB_GAP: f32 = 5.;

const CRUMB_ICON: f32 = 13.;

fn is_alt_click(event: &ClickEvent) -> bool {
    matches!(event, ClickEvent::Mouse(mouse) if mouse.up.modifiers.alt)
}

impl FinderDemo {
    pub(crate) fn path_bar(
        &self,
        palette: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let here = self.folders.get(self.target());
        let crumbs: &[&str] = here.map_or(&[][..], |folder| folder.crumbs.as_slice());
        let chain = self.crumb_ids();
        let named = self.crumb_labels(crumbs, window);
        let mut trail = div()
            .size_full()
            .flex()
            .items_center()
            .gap(px(CRUMB_GAP))
            .pl(px(CRUMB_INSET))
            .pr(px(CRUMB_INSET))
            .text_size(tokens::small_system_font_size())
            .text_color(palette.secondary);

        let (selection, selection_text) = (palette.selection, palette.selection_text);
        for (index, crumb) in crumbs.iter().enumerate() {
            // The fixture starts at the home folder, so the crumbs above it lead nowhere.
            let target = index
                .checked_sub(crumbs.len() - chain.len())
                .and_then(|step| chain.get(step).copied());
            let mut segment = div()
                .id(("ng.finder.crumb", index))
                .h_full()
                .flex()
                .items_center()
                .min_w(px(CRUMB_ICON))
                .gap(px(CRUMB_GAP))
                .active(move |style| style.text_color(selection_text))
                .child(place_art(index == 0, CRUMB_ICON))
                .when(named[index], |crumb_row| {
                    crumb_row.child(
                        div()
                            .min_w(px(0.))
                            .rounded_full()
                            .text_ellipsis()
                            .drag_over::<RowDrag>(move |style, _, _, _| {
                                style.bg(selection).text_color(selection_text)
                            })
                            .child(*crumb),
                    )
                });
            if index + 1 < crumbs.len() {
                if let Some(chevron) = small_glyph(
                    "chevron.right",
                    7.,
                    FontWeight::SEMIBOLD,
                    SystemColor::SecondaryLabel,
                ) {
                    // Finder leaves a point more after a chevron than before it.
                    segment = segment.child(div().flex_none().mr(px(1.)).child(chevron));
                }
            }
            let segment = segment.when_some(target, |crumb_row, id| {
                // ⌥-click opens the crumb in a new window and closes this one.
                crumb_row
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if is_alt_click(event) {
                            crate::open_finder_window(id, 1, cx);
                            window.remove_window();
                        } else {
                            this.go(id, window, cx);
                        }
                    }))
                    .on_drop(
                        cx.listener(move |this, grab: &RowDrag, window: &mut Window, cx| {
                            let keys = this.drag_keys(&grab.key);
                            this.drop_into(id, &keys, window.modifiers().alt, cx);
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
            .bg(BAR_TINT)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .child(native_rule("ng.finder.path.hairline")),
            )
            .child(trail)
            .into_any_element()
    }

    /// Which crumbs keep their name: Finder drops them from the left until the trail fits.
    fn crumb_labels(&self, crumbs: &[&str], window: &Window) -> Vec<bool> {
        let mut named = vec![true; crumbs.len()];
        let size = tokens::small_system_font_size();
        let labels: Vec<f32> = crumbs
            .iter()
            .map(|crumb| text_width(window, crumb.to_string(), size))
            .collect();
        let room = self.list_room - self.pane_w() - 2. * CRUMB_INSET;
        let trail = |named: &[bool]| {
            let icons = crumbs.len() as f32 * (CRUMB_ICON + CRUMB_GAP);
            let chevrons = (crumbs.len().saturating_sub(1)) as f32 * (CHEVRON_W + CRUMB_GAP + 1.);
            let names: f32 = labels
                .iter()
                .zip(named)
                .filter(|(_, keep)| **keep)
                .map(|(width, _)| width + CRUMB_GAP)
                .sum();
            icons + chevrons + names
        };
        for index in 0..named.len().saturating_sub(1) {
            if trail(&named) <= room {
                break;
            }
            named[index] = false;
        }
        named
    }
}
