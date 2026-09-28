use crate::Demo;
use gpui::{AnyElement, App, Context, Hsla, Rgba, SharedString, Window, div, prelude::*, px};
use native_gpui::tokens::{Appearance, SystemColor, system_color};

pub fn set<'a, T: Clone + 'static>(
    cx: &'a mut Context<Demo>,
    apply: fn(&mut Demo, T),
) -> impl Fn(&T, &mut Window, &mut App) + 'static + use<'a, T> {
    cx.listener(move |d, value: &T, _, cx| {
        apply(d, value.clone());
        cx.notify();
    })
}

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
                .child(SharedString::from(title)),
        )
        .children(children)
        .into_any_element()
}

pub fn caption(text: impl Into<SharedString>) -> AnyElement {
    div()
        .text_xs()
        .text_color(system_color(SystemColor::TertiaryLabel))
        .child(text.into())
        .into_any_element()
}

pub fn mono(text: impl Into<SharedString>) -> AnyElement {
    div()
        .font_family("SF Mono")
        .text_xs()
        .text_color(system_color(SystemColor::SecondaryLabel))
        .child(text.into())
        .into_any_element()
}

pub fn note(text: impl Into<SharedString>) -> AnyElement {
    div()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(system_color(SystemColor::QuaternaryLabel))
        .text_xs()
        .text_color(system_color(SystemColor::SecondaryLabel))
        .child(text.into())
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

pub fn tile(label: impl Into<SharedString>, width: f32, child: AnyElement) -> AnyElement {
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
                        .child(SharedString::from(*name)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(system_color(SystemColor::SecondaryLabel))
                        .child(SharedString::from(value)),
                )
        }))
        .into_any_element()
}

pub fn knob(label: &str, value: impl Into<SharedString>, control: AnyElement) -> AnyElement {
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
                .child(SharedString::from(label)),
        )
        .child(div().w(px(180.)).child(control))
        .child(div().w(px(90.)).child(mono(value)))
        .into_any_element()
}

pub fn checker(size_px: f32, child: AnyElement) -> AnyElement {
    let cell = px(size_px / 8.);
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
                .children((0..64).map(|i| {
                    div()
                        .size(cell)
                        .bg(system_color(if (i / 8 + i % 8) % 2 == 0 {
                            SystemColor::Label
                        } else {
                            SystemColor::TertiaryLabel
                        }))
                })),
        )
        .child(div().absolute().inset_0().child(child))
        .into_any_element()
}

pub fn hex(c: Hsla) -> String {
    let c = Rgba::from(c);
    let q = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02X}{:02X}{:02X}{:02X}", q(c.r), q(c.g), q(c.b), q(c.a))
}

pub fn opt_hex(c: Option<Hsla>) -> String {
    c.map_or_else(|| "refused".into(), hex)
}

pub fn appearance_swatch(
    color: SystemColor,
    width: f32,
    label: impl FnOnce(Option<Hsla>, Option<Hsla>) -> String,
) -> AnyElement {
    let light = color.try_resolve(Appearance::Light);
    let dark = color.try_resolve(Appearance::Dark);
    let half = |value: Option<Hsla>| div().flex_1().h_full().when_some(value, |c, v| c.bg(v));

    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .w(px(width))
        .child(checker(
            width,
            div()
                .flex()
                .size_full()
                .child(half(light))
                .child(half(dark))
                .into_any_element(),
        ))
        .child(mono(label(light, dark)))
        .child(caption(format!("{} {}", opt_hex(light), opt_hex(dark))))
        .into_any_element()
}
