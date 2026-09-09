//! Hosted: the toolbar band's material sits above the whole GPUI layer and would cover it.

use gpui::{AnyElement, Context, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{ControlSize, SegmentStyle, native_label, native_segmented};

use crate::app::FinderDemo;

pub(crate) const SCOPE_H: f32 = 28.;
const SCOPE_INSET: f32 = 8.;
const SCOPE_LABEL_W: f32 = 46.;
const SCOPE_GAP: f32 = 6.;

pub(crate) const THIS_MAC: usize = 0;
pub(crate) const THIS_FOLDER: usize = 1;
pub(crate) const NETWORK: usize = 2;

impl FinderDemo {
    pub(crate) fn scope_name(&self) -> String {
        match self.scope {
            THIS_MAC => "This Mac".to_string(),
            NETWORK => "Network".to_string(),
            _ => self
                .folder()
                .map_or_else(|| "This Mac".to_string(), |folder| folder.title.to_string()),
        }
    }

    pub(crate) fn scope_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let folder_name = self.folder().map_or("this Mac".to_string(), |folder| {
            format!("\u{201c}{}\u{201d}", folder.title)
        });
        let this = cx.entity().downgrade();
        div()
            .h(px(SCOPE_H))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .pl(px(SCOPE_INSET))
            .child(
                div()
                    .flex_none()
                    .w(px(SCOPE_LABEL_W))
                    .mr(px(SCOPE_GAP))
                    .child(
                        native_label("ng.finder.scope.label", "Search:")
                            .font_size(tokens::small_system_font_size())
                            .color(SystemColor::SecondaryLabel),
                    ),
            )
            .child(
                native_segmented("ng.finder.scope.segments", self.scope)
                    .segments(["This Mac".to_string(), folder_name, "Network".to_string()])
                    .style(SegmentStyle::Capsule)
                    .control_size(ControlSize::Small)
                    .selected_bezel_color(SystemColor::UnemphasizedSelectedContentBackground)
                    .on_change(move |index, _, cx| {
                        let index = *index;
                        this.update(cx, |this, cx| this.set_scope(index, cx)).ok();
                    }),
            )
            .into_any_element()
    }
}
