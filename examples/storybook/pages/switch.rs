use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window, prelude::*};
use native_gpui::native_switch;

pub struct State {
    pub on: bool,
    pub second: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            on: true,
            second: false,
        }
    }
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let (on, second) = (d.switch.on, d.switch.second);

    stack(vec![
        demo(
            "state",
            native_switch("live", on)
                .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                    d.switch.on = *value;
                    cx.notify();
                }))
                .into_any_element(),
        ),
        row(
            "two independent switches",
            vec![
                native_switch("a", on)
                    .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                        d.switch.on = *value;
                        cx.notify();
                    }))
                    .into_any_element(),
                native_switch("b", second)
                    .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                        d.switch.second = *value;
                        cx.notify();
                    }))
                    .into_any_element(),
            ],
        ),
        demo(
            "disabled",
            native_switch("off", on).enabled(false).into_any_element(),
        ),
        props(&[
            ("native_switch(id, on)", on.to_string()),
            ("on_change(Fn(&bool, ..))", "wired".into()),
            ("enabled(bool)", "false on the last one".into()),
        ]),
    ])
}
