//! Layout helpers every page shares.

use gpui::{AnyElement, SharedString, div, prelude::*, px};
use native_gpui::tokens::{SystemColor, system_color};

pub fn stack(children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_6()
        .children(children)
        .into_any_element()
}

pub fn sized(width: f32, child: AnyElement) -> AnyElement {
    div().w(px(width)).child(child).into_any_element()
}

pub fn demo(label: &str, child: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(caption(label.to_uppercase()))
        .child(child)
        .into_any_element()
}

pub fn row(label: &str, children: Vec<AnyElement>) -> AnyElement {
    demo(
        label,
        div()
            .flex()
            .items_center()
            .gap_4()
            .flex_wrap()
            .children(children)
            .into_any_element(),
    )
}

pub fn section(title: &str, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .text_color(system_color(SystemColor::Label))
                .child(SharedString::from(title.to_string())),
        )
        .children(children)
        .into_any_element()
}

pub fn caption(text: impl Into<String>) -> AnyElement {
    div()
        .text_xs()
        .text_color(system_color(SystemColor::TertiaryLabel))
        .child(SharedString::from(text.into()))
        .into_any_element()
}

pub fn mono(text: impl Into<String>) -> AnyElement {
    div()
        .font_family("SF Mono")
        .text_xs()
        .text_color(system_color(SystemColor::SecondaryLabel))
        .child(SharedString::from(text.into()))
        .into_any_element()
}

pub fn note(text: impl Into<String>) -> AnyElement {
    div()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(system_color(SystemColor::QuaternaryLabel))
        .text_xs()
        .text_color(system_color(SystemColor::SecondaryLabel))
        .child(SharedString::from(text.into()))
        .into_any_element()
}

pub fn grid(children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap_3()
        .children(children)
        .into_any_element()
}

pub fn tile(label: impl Into<String>, width: f32, child: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .w(px(width))
        .child(child)
        .child(mono(label))
        .into_any_element()
}

pub fn props(rows: &[(&str, String)]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .child(div().pb_2().child(caption("PROPERTIES")))
        .children(rows.iter().map(|(name, value)| {
            div()
                .flex()
                .gap_4()
                .py_1p5()
                .border_b_1()
                .border_color(system_color(SystemColor::Separator))
                .child(
                    div()
                        .w(px(240.))
                        .font_family("SF Mono")
                        .text_sm()
                        .text_color(system_color(SystemColor::Label))
                        .child(SharedString::from(name.to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(system_color(SystemColor::SecondaryLabel))
                        .child(SharedString::from(value.clone())),
                )
        }))
        .into_any_element()
}

pub fn knob(label: &str, value: impl Into<String>, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .py_1()
        .child(
            div()
                .w(px(150.))
                .text_xs()
                .text_color(system_color(SystemColor::SecondaryLabel))
                .child(SharedString::from(label.to_string())),
        )
        .child(div().w(px(180.)).child(control))
        .child(div().w(px(90.)).child(mono(value)))
        .into_any_element()
}

pub fn checker(size_px: f32, child: AnyElement) -> AnyElement {
    let cells: Vec<AnyElement> = (0..64)
        .map(|i| {
            let dark = ((i / 8) + (i % 8)) % 2 == 0;
            div()
                .w(px(size_px / 8.))
                .h(px(size_px / 8.))
                .bg(if dark {
                    system_color(SystemColor::Label)
                } else {
                    system_color(SystemColor::TertiaryLabel)
                })
                .into_any_element()
        })
        .collect();

    div()
        .relative()
        .size(px(size_px))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_wrap()
                .w(px(size_px))
                .children(cells),
        )
        .child(div().absolute().inset_0().child(child))
        .into_any_element()
}
