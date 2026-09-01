use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window, div, prelude::*};
use native_gpui::{TrackingMode, native_segmented};

const STAGES: [&str; 3] = ["Design", "Build", "Ship"];
const SIDES: [&str; 2] = ["Left", "Right"];
const DAYS: [&str; 5] = ["Mon", "Tue", "Wed", "Thu", "Fri"];
const ACTIONS: [&str; 3] = ["Undo", "Redo", "Revert"];

pub struct State {
    pub selected: usize,
    pub pair: usize,
    pub many: usize,
    pub pressed: Option<usize>,
    pub presses: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selected: 1,
            pair: 0,
            many: 2,
            pressed: None,
            presses: 0,
        }
    }
}

fn name(labels: &[&'static str], index: usize) -> &'static str {
    labels.get(index).copied().unwrap_or("none")
}

fn labelled(control: AnyElement, value: impl Into<String>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(control)
        .child(mono(value))
        .into_any_element()
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let s = &d.segmented;
    let (selected, pair, many) = (s.selected, s.pair, s.many);
    let (pressed, presses) = (s.pressed, s.presses);

    stack(vec![
        demo(
            "select one",
            labelled(
                native_segmented("stages", selected)
                    .segments(STAGES)
                    .on_change(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                        d.segmented.selected = *index;
                        cx.notify();
                    }))
                    .into_any_element(),
                format!("selected = {selected} ({})", name(&STAGES, selected)),
            ),
        ),
        section(
            "Item counts",
            vec![
                div()
                    .flex()
                    .items_start()
                    .gap_8()
                    .child(labelled(
                        native_segmented("pair", pair)
                            .segments(SIDES)
                            .on_change(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                                d.segmented.pair = *index;
                                cx.notify();
                            }))
                            .into_any_element(),
                        format!("2 items, {pair}"),
                    ))
                    .child(labelled(
                        native_segmented("many", many)
                            .segments(DAYS)
                            .on_change(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                                d.segmented.many = *index;
                                cx.notify();
                            }))
                            .into_any_element(),
                        format!("5 items, {many} ({})", name(&DAYS, many)),
                    ))
                    .into_any_element(),
                caption("Width comes from fittingSize, so it follows the labels."),
            ],
        ),
        demo(
            "momentary",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    native_segmented("actions", 0)
                        .segments(ACTIONS)
                        .tracking_mode(TrackingMode::Momentary)
                        .on_change(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                            d.segmented.pressed = Some(*index);
                            d.segmented.presses += 1;
                            cx.notify();
                        }))
                        .into_any_element(),
                )
                .child(mono("selectedSegment = -1, always"))
                .child(mono(match pressed {
                    Some(index) => {
                        format!("{presses} presses, last {}", name(&ACTIONS, index))
                    }
                    None => "no presses yet".into(),
                }))
                .child(caption(
                    "Nothing stays lit, and there is no selection to read — setSelectedSegment \
                     never sticks in this mode, so the 0 passed in is inert. The clicked index \
                     still arrives, because the host reads it while the AppKit action is on the \
                     stack.",
                ))
                .into_any_element(),
        ),
        demo(
            "select any",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    native_segmented("any", 0)
                        .segments(DAYS)
                        .tracking_mode(TrackingMode::SelectAny)
                        .into_any_element(),
                )
                .child(caption(
                    "AppKit holds several segments at once here, and on_change reports a single \
                     index, which cannot describe that. So paint leaves the selection alone in \
                     this mode — the 0 passed in is never written. It has to be: a per-paint \
                     write would pin that segment on where the user could not switch it off, and \
                     an out-of-range index would deselect everything again every frame. Reach for \
                     several native_checkbox instead.",
                ))
                .into_any_element(),
        ),
        row(
            "nothing selected",
            vec![
                native_segmented("unselected", usize::MAX)
                    .segments(STAGES)
                    .into_any_element(),
                caption("An out-of-range index becomes AppKit's -1, not segment 0."),
            ],
        ),
        row(
            "disabled",
            vec![
                native_segmented("disabled", selected)
                    .segments(STAGES)
                    .enabled(false)
                    .into_any_element(),
                native_segmented("disabled-momentary", 0)
                    .segments(ACTIONS)
                    .tracking_mode(TrackingMode::Momentary)
                    .enabled(false)
                    .into_any_element(),
            ],
        ),
        props(&[
            ("native_segmented(id, selected)", selected.to_string()),
            ("segments(iter)", STAGES.join(", ")),
            (
                "tracking_mode(..)",
                "SelectOne (default) | SelectAny | Momentary".into(),
            ),
            ("enabled(bool)", "false on the last pair".into()),
            (
                "on_change(Fn(&usize, ..))",
                format!("clicked index; {presses} momentary presses so far"),
            ),
            (
                "value accessor",
                "ValueKind::Int, so integerValue — and here that genuinely is the selection: it \
                 tracks selectedSegment exactly. The pop-up next door reads the same accessor as \
                 constant cell state and needs indexOfSelectedItem (src/bridge/host.rs)"
                    .into(),
            ),
        ]),
    ])
}
