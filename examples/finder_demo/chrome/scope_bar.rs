//! Hosted: the toolbar band's material sits above the whole GPUI layer and would cover it.

use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use native_gpui::tokens::SystemColor;
use native_gpui::{ButtonBezel, ControlSize, native_button, native_label};
use objc2_app_kit::{NSFont, NSTextField};
use objc2_foundation::NSString;

use crate::app::FinderDemo;
use crate::chrome::criteria_bar::Criterion;
use crate::data;

pub(crate) const SCOPE_H: f32 = 32.;
const LABEL_INSET: f32 = 8.;
const LABEL_GAP: f32 = 4.;
const BUTTON_GAP: f32 = 2.;
const TRAILING_INSET: f32 = 10.;
const FONT_SIZE: f32 = 12.;
const LABEL: &str = "Search:";

pub(crate) const THIS_MAC: usize = 0;
pub(crate) const THIS_FOLDER: usize = 1;
pub(crate) const NETWORK: usize = 2;

fn label_width() -> f32 {
    native_gpui::fitting_size("NSTextField.scope_label", |mtm| {
        let label = NSTextField::labelWithString(&NSString::from_str(LABEL), mtm);
        label.setFont(Some(&NSFont::boldSystemFontOfSize(FONT_SIZE.into())));
        label.sizeToFit();
        label.frame().size
    })
    .width as f32
}

impl FinderDemo {
    pub(crate) fn scope_name(&self) -> String {
        match self.scope {
            THIS_MAC => finder_fs::words::say("This Mac").into_owned(),
            NETWORK => finder_fs::words::say("Network").into_owned(),
            _ => self
                .folder()
                .map_or_else(|| "This Mac".to_string(), |folder| folder.title.to_string()),
        }
    }

    pub(crate) fn scope_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let folder_name = self.folder().map_or_else(
            || "this Mac".to_string(),
            |folder| format!("\u{201c}{}\u{201d}", folder.title),
        );
        let mut scopes = vec![
            (THIS_MAC, finder_fs::words::say("This Mac").into_owned()),
            (THIS_FOLDER, folder_name),
        ];
        if !data::network_roots().is_empty() {
            scopes.push((NETWORK, finder_fs::words::say("Network").into_owned()));
        }
        let (toggle, symbol) = if self.criteria.is_empty() {
            ("show search criteria", "plus")
        } else {
            ("hide search criteria", "minus")
        };
        div()
            .h(px(SCOPE_H))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .child(
                div()
                    .flex_none()
                    .ml(px(LABEL_INSET))
                    .w(px(label_width()))
                    .child(
                        native_label("ng.finder.scope.label", LABEL)
                            .font_size(px(FONT_SIZE))
                            .weight(FontWeight::BOLD)
                            .color(SystemColor::SecondaryLabel),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .ml(px(LABEL_GAP))
                    .flex()
                    .gap(px(BUTTON_GAP))
                    .children(scopes.into_iter().map(|(scope, title)| {
                        native_button(("ng.finder.scope", scope), title)
                            .bezel_style(ButtonBezel::AccessoryBar)
                            .control_size(ControlSize::Small)
                            .font_size(px(FONT_SIZE))
                            .weight(FontWeight::BOLD)
                            .on(self.scope == scope)
                            .border_on_hover(true)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_scope(scope, cx)))
                    })),
            )
            .child(div().flex_1())
            .child(
                native_button("ng.finder.scope.save", "Save")
                    .bezel_style(ButtonBezel::AccessoryBarAction)
                    .control_size(ControlSize::Small)
                    .font_size(px(FONT_SIZE))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_sheet = Some("New Smart Folder".into());
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .ml(px(BUTTON_GAP))
                    .mr(px(TRAILING_INSET))
                    .child(
                        native_button("ng.finder.scope.criteria", toggle)
                            .symbol(symbol)
                            .bezel_style(ButtonBezel::AccessoryBarAction)
                            .control_size(ControlSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_criteria(cx))),
                    ),
            )
            .into_any_element()
    }

    fn toggle_criteria(&mut self, cx: &mut Context<Self>) {
        if self.criteria.is_empty() {
            self.criteria.push(Criterion::default());
        } else {
            self.criteria.clear();
        }
        self.run_search(cx);
    }
}
