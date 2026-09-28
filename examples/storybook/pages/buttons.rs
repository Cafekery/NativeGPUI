use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window, div, prelude::*};
use native_gpui::{ButtonBezel, CheckState, native_button, native_checkbox, native_radio};

pub struct State {
    pub saves: usize,
    pub cancels: usize,
    pub drafts: CheckState,
    pub wrap: bool,
    pub size: usize,
    pub align: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            saves: 0,
            cancels: 0,
            drafts: CheckState::Mixed,
            wrap: true,
            size: 1,
            align: 0,
        }
    }
}

const BEZELS: &[(ButtonBezel, &str)] = &[
    (ButtonBezel::Automatic, "Automatic"),
    (ButtonBezel::Push, "Push"),
    (ButtonBezel::FlexiblePush, "FlexiblePush"),
    (ButtonBezel::SmallSquare, "SmallSquare"),
    (ButtonBezel::Toolbar, "Toolbar"),
    (ButtonBezel::AccessoryBarAction, "AccessoryBarAction"),
    (ButtonBezel::AccessoryBar, "AccessoryBar"),
    (ButtonBezel::Glass, "Glass"),
    (ButtonBezel::Circular, "Circular"),
    (ButtonBezel::HelpButton, "HelpButton"),
    (ButtonBezel::Badge, "Badge"),
    (ButtonBezel::Disclosure, "Disclosure"),
    (ButtonBezel::PushDisclosure, "PushDisclosure"),
];

const SIZES: &[&str] = &["Small", "Medium", "Large"];
const ALIGN: &[&str] = &["Leading", "Centre", "Trailing"];

// Each hosted element gets its own clip wrapper, so AppKit cannot auto-group these radios itself.
fn radio_group(
    id: &'static str,
    titles: &'static [&'static str],
    selected: usize,
    set: fn(&mut Demo, usize),
    cx: &mut Context<Demo>,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_4()
        .children(titles.iter().enumerate().map(|(index, title)| {
            native_radio((id, index), selected == index)
                .title(*title)
                .on_change(cx.listener(move |d: &mut Demo, on: &bool, _, cx| {
                    if *on {
                        set(d, index);
                        cx.notify();
                    }
                }))
        }))
        .into_any_element()
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let State {
        saves,
        cancels,
        drafts,
        wrap,
        size,
        align,
    } = d.buttons;

    stack(vec![
        row(
            "push — the counters prove the handler fires",
            vec![
                native_button("save", "Save")
                    .default_button(true)
                    .on_click(set(cx, |d, ()| d.buttons.saves += 1))
                    .into_any_element(),
                mono(saves.to_string()),
                native_button("cancel", "Cancel")
                    .on_click(set(cx, |d, ()| d.buttons.cancels += 1))
                    .into_any_element(),
                mono(cancels.to_string()),
                native_button("push-off", "Disabled")
                    .enabled(false)
                    .into_any_element(),
            ],
        ),
        note(
            "Return activates Save. `default_button` is asked of NSWindow.defaultButtonCell, not \
             of keyEquivalent(): a button set to \"\\r\" reads that property back as the empty \
             string. AppKit resolves Return to the first default it finds, so set it on at most \
             one button per window.",
        ),
        section(
            "bezel styles",
            vec![
                caption("Automatic lets AppKit pick from the button's contents and its position."),
                grid(
                    BEZELS
                        .iter()
                        .enumerate()
                        .map(|(index, (bezel, name))| {
                            tile(
                                *name,
                                160.,
                                native_button(("bezel", index), "Action")
                                    .bezel_style(*bezel)
                                    .into_any_element(),
                            )
                        })
                        .collect(),
                ),
            ],
        ),
        section(
            "checkbox",
            vec![
                row(
                    "two state",
                    vec![
                        native_checkbox("wrap", wrap)
                            .title("Wrap lines")
                            .on_change(set(cx, |d, state| d.buttons.wrap = state == CheckState::On))
                            .into_any_element(),
                        mono(if wrap { "On" } else { "Off" }),
                    ],
                ),
                row(
                    "allows_mixed — AppKit cycles off, mixed, on",
                    vec![
                        native_checkbox("drafts", drafts == CheckState::On)
                            .title("Include drafts")
                            .allows_mixed(true)
                            .state(drafts)
                            .on_change(set(cx, |d, state| d.buttons.drafts = state))
                            .into_any_element(),
                        mono(format!("{drafts:?}")),
                    ],
                ),
                row(
                    "state(..), disabled",
                    vec![
                        native_checkbox("cb-off", false)
                            .title("Off")
                            .enabled(false)
                            .into_any_element(),
                        native_checkbox("cb-on", true)
                            .title("On")
                            .enabled(false)
                            .into_any_element(),
                        native_checkbox("cb-mixed", false)
                            .state(CheckState::Mixed)
                            .title("Mixed")
                            .enabled(false)
                            .into_any_element(),
                    ],
                ),
            ],
        ),
        section(
            "radio",
            vec![
                demo(
                    "one group",
                    radio_group("size", SIZES, size, |d, index| d.buttons.size = index, cx),
                ),
                demo(
                    "a second group, independent of the first",
                    radio_group(
                        "align",
                        ALIGN,
                        align,
                        |d, index| d.buttons.align = index,
                        cx,
                    ),
                ),
                row(
                    "disabled",
                    vec![
                        native_radio("radio-off-a", true)
                            .title("Selected")
                            .enabled(false)
                            .into_any_element(),
                        native_radio("radio-off-b", false)
                            .title("Not selected")
                            .enabled(false)
                            .into_any_element(),
                    ],
                ),
            ],
        ),
        props(&[
            (
                "native_button(id, title).on_click",
                format!("{saves} + {cancels} clicks"),
            ),
            (
                "default_button(bool)",
                "asked of NSWindow.defaultButtonCell".into(),
            ),
            (
                "bezel_style(ButtonBezel)",
                format!("{} styles above", BEZELS.len()),
            ),
            ("native_checkbox(id, checked)", format!("wrap = {wrap}")),
            ("state(CheckState)", format!("{drafts:?}")),
            (
                "allows_mixed(bool)",
                "true on Include drafts; Mixed implies it".into(),
            ),
            (
                "native_radio(id, selected)",
                format!("size {size}, align {align} — the page owns both"),
            ),
            ("enabled(bool)", "false on the last of each band".into()),
        ]),
    ])
}
